//! What Main.qml and ApprovalDialog.qml see: the questions waiting, the
//! answer, and whether an agent lets this helper answer now.

use std::pin::Pin;
use std::sync::atomic::Ordering;

use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        include!("qt_app.h");
        type QString = cxx_qt_lib::QString;

        #[rust_name = "create_widgets_application"]
        unsafe fn create_widgets_application();
        #[rust_name = "exec_widgets_application"]
        unsafe fn exec_widgets_application() -> i32;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        type ApprovalBridge = super::ApprovalBridgeRust;

        /// The questions waiting, as JSON (pending.rs).
        #[qinvokable]
        fn approvals(self: &ApprovalBridge) -> QString;

        /// Answers question `id` (pending.rs).
        #[qinvokable]
        fn answer_approval(self: Pin<&mut ApprovalBridge>, id: i32, accept: bool);

        /// Whether an agent lets this helper answer now.
        #[qinvokable]
        fn connected(self: &ApprovalBridge) -> bool;
    }
}

#[derive(Default)]
pub struct ApprovalBridgeRust;

impl qobject::ApprovalBridge {
    pub fn approvals(&self) -> QString {
        QString::from(&crate::pending::pending_json())
    }

    pub fn answer_approval(self: Pin<&mut Self>, id: i32, accept: bool) {
        crate::pending::answer(id.max(0) as u64, accept);
    }

    pub fn connected(&self) -> bool {
        crate::helper::CONNECTED.load(Ordering::SeqCst)
    }
}
