//! Windows permissions of the server's data folder and its secret files.
//!
//! The folder (`%ProgramData%\Lunaris\Server`, service.rs) is for SYSTEM and
//! Administrators only, owned by Administrators, with nothing inherited from
//! ProgramData (where every user may create folders), as the agent's service
//! does for its own folder. This mirrors `secure_data_dir` in
//! agent/src/win_service.rs (same SDDL, same checks); it is not shared
//! because the agent's copy lives in its service module and `common` carries
//! no Windows dependencies.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::FromRawHandle;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SetNamedSecurityInfoW,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, IsWellKnownSid,
    WinBuiltinAdministratorsSid, WinLocalSystemSid, ACL, DACL_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, SE_DACL_PROTECTED,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};

/// The folder: inherited by everything created in it.
const DATA_DIR_SDDL: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// Files already in it.
const DATA_FILE_SDDL: &str = "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
/// A secret file written by `write_private`: SYSTEM, Administrators and
/// its owner (OW, the account that runs the server: also a plain user
/// running it from a console with its data in its own folder).
const PRIVATE_FILE_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)";

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

fn wide_z(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn win32_error(e: &windows::core::Error) -> u32 {
    let code = e.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}

/// A link, junction or any other reparse point.
fn is_reparse_point(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// A self-relative security descriptor made from SDDL (freed with LocalFree).
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> Result<Self, String> {
        let wide = wide_z(OsStr::new(sddl));
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(wide.as_ptr()), SDDL_REVISION_1, &mut sd, None)
        }
        .map_err(|e| format!("security descriptor {sddl}: {e}"))?;
        Ok(Self(sd))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0 .0,
            bInheritHandle: false.into(),
        }
    }

    /// Sets this owner and (protected) DACL on `path`.
    fn apply(&self, path: &Path) -> Result<(), String> {
        unsafe {
            let mut owner = PSID::default();
            let mut defaulted = windows::core::BOOL::default();
            GetSecurityDescriptorOwner(self.0, &mut owner, &mut defaulted).map_err(|e| e.to_string())?;
            let mut present = windows::core::BOOL::default();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted).map_err(|e| e.to_string())?;
            let wide = wide_z(path.as_os_str());
            let result = SetNamedSecurityInfoW(
                PCWSTR(wide.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                Some(owner),
                None,
                Some(dacl as *const ACL),
                None,
            );
            if result != ERROR_SUCCESS {
                return Err(format!(
                    "cannot set the permissions of {}: {}",
                    path.display(),
                    std::io::Error::from_raw_os_error(result.0 as i32)
                ));
            }
        }
        Ok(())
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Creates `dir` for SYSTEM and Administrators only, or takes an existing
/// one over (owner and permissions reset, also of what is in it). Refuses a
/// folder that is, or holds, a link or other reparse point.
pub fn secure_dir(dir: &Path) -> Result<(), String> {
    let dir_sd = SecurityDescriptor::from_sddl(DATA_DIR_SDDL)?;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if is_reparse_point(&m) => {
            return Err(format!("{} is a link: refusing to use it, remove it", dir.display()));
        }
        Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", dir.display())),
        Ok(_) => {}
        Err(_) => {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            // Created with its permissions, so nobody can open it in between.
            let attributes = dir_sd.attributes();
            let wide = wide_z(dir.as_os_str());
            if let Err(e) = unsafe { CreateDirectoryW(PCWSTR(wide.as_ptr()), Some(&attributes as *const _)) } {
                if win32_error(&e) != ERROR_ALREADY_EXISTS.0 {
                    return Err(format!("cannot create {}: {e}", dir.display()));
                }
            }
            // Made by someone else in between: checked like any existing one.
            let m = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            if is_reparse_point(&m) || !m.is_dir() {
                return Err(format!("{} is not a plain folder: refusing to use it", dir.display()));
            }
        }
    }
    apply_or_already_locked(&dir_sd, dir)?;
    let file_sd = SecurityDescriptor::from_sddl(DATA_FILE_SDDL)?;
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let m = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if is_reparse_point(&m) {
            return Err(format!("{} is a link: refusing to use {}, remove it", path.display(), dir.display()));
        }
        if m.is_dir() {
            apply_or_already_locked(&dir_sd, &path)?;
        } else {
            apply_or_already_locked(&file_sd, &path)?;
        }
    }
    Ok(())
}

/// `sd.apply(path)`; when that fails, accepted only if `path` already has
/// the owner (Administrators or SYSTEM) and a protected DACL.
fn apply_or_already_locked(sd: &SecurityDescriptor, path: &Path) -> Result<(), String> {
    match sd.apply(path) {
        Ok(()) => Ok(()),
        Err(_) if owned_by_admins(path) && dacl_protected(path) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The owner and the security descriptor of `path` (None when unreadable).
fn owner_of(path: &Path) -> Option<(bool, SecurityDescriptor)> {
    unsafe {
        let wide = wide_z(path.as_os_str());
        let mut owner = PSID::default();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let result = GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner as *mut PSID),
            None,
            None,
            None,
            &mut sd as *mut PSECURITY_DESCRIPTOR,
        );
        if result != ERROR_SUCCESS || sd.is_invalid() {
            return None;
        }
        let sd = SecurityDescriptor(sd);
        let admins = !owner.is_invalid()
            && (IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool()
                || IsWellKnownSid(owner, WinLocalSystemSid).as_bool());
        Some((admins, sd))
    }
}

/// Owned by Administrators or SYSTEM: made by the service or an
/// administrator, not planted by a user.
pub fn owned_by_admins(path: &Path) -> bool {
    owner_of(path).map(|(admins, _)| admins).unwrap_or(false)
}

/// An existing plain folder (no link) owned by Administrators or SYSTEM
/// with nothing inherited: as `secure_dir` leaves one.
pub fn is_locked_dir(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Ok(m) if m.is_dir() && !is_reparse_point(&m))
        && owned_by_admins(path)
        && dacl_protected(path)
}

fn dacl_protected(path: &Path) -> bool {
    let Some((_, sd)) = owner_of(path) else { return false };
    let mut control = 0u16;
    let mut revision = 0u32;
    let read = unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision).is_ok() };
    read && control & SE_DACL_PROTECTED.0 != 0
}

/// Writes `contents` to a new file made with `PRIVATE_FILE_SDDL` next to
/// `path`, then puts it in `path`'s place (the permissions go with it).
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let sd = SecurityDescriptor::from_sddl(PRIVATE_FILE_SDDL).map_err(std::io::Error::other)?;
    let attributes = sd.attributes();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", crate::security::random_hex(6)));
    let wide = wide_z(tmp.as_os_str());
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Foundation::GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            Some(&attributes as *const SECURITY_ATTRIBUTES),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| std::io::Error::from_raw_os_error(win32_error(&e) as i32))?;
    let written = {
        let mut file = unsafe { std::fs::File::from_raw_handle(handle.0) };
        file.write_all(contents).and_then(|_| file.sync_all())
    };
    if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}
