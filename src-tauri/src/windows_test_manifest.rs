// Tauri's application manifest is not linked into Cargo's library unit-test
// executable. Its dialog imports still need Common-Controls v6, otherwise the
// Windows loader fails before any tests can run (STATUS_ENTRYPOINT_NOT_FOUND).
// Cargo's rustc-link-arg-tests only covers integration targets, so emit the
// MSVC linker directive inside the cfg(test)-only object instead. This module
// is absent from production builds and never changes the application manifest.
const DIRECTIVE: &[u8] = b" /MANIFESTDEPENDENCY:\"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'\" ";

#[used]
#[link_section = ".drectve"]
static TEST_MANIFEST: [u8; DIRECTIVE.len()] = *b" /MANIFESTDEPENDENCY:\"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'\" ";
