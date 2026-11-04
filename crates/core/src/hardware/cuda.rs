//! Query a driver's CUDA API capability, not an installed toolkit or a GPU test.
//! Production callers run only in the time-bounded Linux hardware helper.
use std::ffi::CStr;

#[cfg(target_os = "linux")]
pub(super) fn driver_version() -> Option<i32> {
    query(c"libcuda.so.1")
}

fn query(name: &CStr) -> Option<i32> {
    struct Library(*mut libc::c_void);
    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: this owns the one successful dlopen reference.
            unsafe { libc::dlclose(self.0) };
        }
    }
    // SAFETY: name is NUL-terminated; keep the handle until after the call.
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return None;
    }
    let library = Library(handle);
    // SAFETY: the handle is live and the symbol name is NUL-terminated.
    let entry = unsafe { libc::dlsym(library.0, c"cuDriverGetVersion".as_ptr()) };
    if entry.is_null() {
        return None;
    }
    // SAFETY: NVIDIA specifies this symbol's C ABI as CUresult(int *).
    let version: unsafe extern "C" fn(*mut libc::c_int) -> libc::c_int =
        unsafe { std::mem::transmute(entry) };
    let mut value = 0;
    // SAFETY: the writable integer and loaded function remain live for the call.
    let result = unsafe { version(&mut value) };
    (result == 0 && value > 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{ensure, Result};
    use std::{ffi::CString, os::unix::ffi::OsStrExt, process::Command};

    fn fixture(body: &str) -> Result<(tempfile::TempDir, CString)> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("driver.c");
        let library = root.path().join("driver.so");
        std::fs::write(&source, body)?;
        let flag = if cfg!(target_os = "macos") {
            "-dynamiclib"
        } else {
            "-shared"
        };
        let compiled = Command::new("cc")
            .args([flag, "-fPIC"])
            .arg(&source)
            .arg("-o")
            .arg(&library)
            .output()?;
        ensure!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let name = CString::new(library.as_os_str().as_bytes())?;
        Ok((root, name))
    }

    #[test]
    fn driver_symbol_reports_capability_without_initializing_cuda() -> Result<()> {
        // The real loaded fixture has no cuInit or CUDA runtime symbols.
        let (_root, name) = fixture("int cuDriverGetVersion(int *v) { *v = 12020; return 0; }")?;
        assert_eq!(query(&name), Some(12020));
        Ok(())
    }

    #[test]
    fn missing_library_symbol_errors_and_invalid_versions_remain_unknown() -> Result<()> {
        let root = tempfile::tempdir()?;
        let missing = CString::new(root.path().join("missing.so").as_os_str().as_bytes())?;
        assert_eq!(query(&missing), None);
        for body in [
            "int otherSymbol(void) { return 0; }",
            "int cuDriverGetVersion(int *v) { *v = 12020; return 3; }",
            "int cuDriverGetVersion(int *v) { *v = 0; return 0; }",
            "int cuDriverGetVersion(int *v) { *v = -1; return 0; }",
        ] {
            let (_root, name) = fixture(body)?;
            assert_eq!(query(&name), None);
        }
        Ok(())
    }
}
