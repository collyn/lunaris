# Removes from a windeployqt'ed folder what Lunaris does not use. Run it
# before Lunaris' own FFmpeg DLLs are copied in, and run smoke-qml.ps1 on the
# finished folder.
#
# usage: prune-qt.ps1 -Dir <package folder>
#
# What goes, and why it is safe (measured on the 0.3.4 client package, 180 MB
# unpacked -> 86 MB; H.264/HEVC with D3D11 present and the CPU fallback were
# run from the pruned folder):
# - vc_redist.x64.exe (24 MB): the CRT DLLs are copied next to the exe instead.
# - opengl32sw.dll (20 MB): Mesa's software OpenGL. Qt Quick runs on Direct3D 11
#   here; without a GPU driver Windows provides WARP for Direct3D.
# - Qt Multimedia's FFmpeg backend and the FFmpeg 7 it brings (avcodec-61 and
#   friends, 17 MB). Lunaris uses Qt Multimedia only for QVideoSink/VideoOutput
#   (the CPU fallback), which the small "windows" backend serves. Lunaris' own
#   FFmpeg 8 DLLs are copied after this script runs.
# - dxcompiler.dll, dxil.dll (13 MB): the Direct3D 12 shader compiler. Qt's
#   shaders are precompiled and Lunaris compiles its own with D3DCompile.
# - d3dcompiler_47.dll (4.5 MB): part of Windows since 8.1 (System32).
# - OpenSSL and its Qt TLS backend, opus.dll: nothing imports them (TLS is
#   SChannel, Opus is linked statically).
# - Qt Quick Controls styles other than Windows (the default here), its
#   fallbacks Fusion and Basic; QML modules no file imports; QML debugging
#   plugins; *.qmltypes (editor metadata).
param([Parameter(Mandatory = $true)][string]$Dir)
$ErrorActionPreference = 'Stop'

$before = (Get-ChildItem $Dir -Recurse -File | Measure-Object Length -Sum).Sum

$files = @(
  'vc_redist*.exe', 'opengl32sw.dll', 'dxcompiler.dll', 'dxil.dll', 'd3dcompiler_47.dll',
  'libcrypto-*.dll', 'libssl-*.dll', 'legacy.dll', 'opus.dll',
  'avcodec-*.dll', 'avformat-*.dll', 'avutil-*.dll', 'avdevice-*.dll', 'avfilter-*.dll',
  'swscale-*.dll', 'swresample-*.dll',
  'multimedia/ffmpegmediaplugin.dll', 'tls/qopensslbackend.dll',
  'Qt6QuickControls2Imagine*.dll', 'Qt6QuickControls2Material*.dll', 'Qt6QuickControls2Universal*.dll',
  'Qt6QuickControls2FluentWinUI3*.dll'
)
$dirs = @(
  'qmltooling',
  'qml/QtQuick/Controls/Imagine', 'qml/QtQuick/Controls/Material', 'qml/QtQuick/Controls/Universal',
  'qml/QtQuick/Controls/FluentWinUI3',
  'qml/QtQuick/Dialogs', 'qml/QtQuick/Particles', 'qml/QtQuick/LocalStorage', 'qml/QtQuick/Effects'
)
foreach ($f in $files) { Remove-Item (Join-Path $Dir $f) -Force -ErrorAction SilentlyContinue }
foreach ($d in $dirs) { Remove-Item (Join-Path $Dir $d) -Recurse -Force -ErrorAction SilentlyContinue }
Get-ChildItem $Dir -Recurse -Filter '*.qmltypes' | Remove-Item -Force

# The Visual C++ runtime next to the exe (a few hundred KB each).
Copy-Item "$env:VCToolsRedistDir/x64/Microsoft.VC143.CRT/*.dll" -Destination $Dir

$after = (Get-ChildItem $Dir -Recurse -File | Measure-Object Length -Sum).Sum
"{0}: {1:N0} MB -> {2:N0} MB" -f $Dir, ($before / 1MB), ($after / 1MB)
