//! Move files to the Windows Recycle Bin through the shell.

use std::path::Path;

use anyhow::{Result, bail};

/// Send `path` to the Recycle Bin. The shell handles the move so the file can
/// still be restored from Explorer; the operation is silent because the viewer
/// already asked for confirmation.
pub fn move_to_recycle_bin(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            SHFILEOPSTRUCTW, SHFileOperationW,
        };

        let from = wide_path(path)?;
        let mut operation = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
            ..Default::default()
        };

        // SAFETY: `from` is a live, double-NUL-terminated UTF-16 buffer and the
        // struct points only at it for the duration of the call.
        let code = unsafe { SHFileOperationW(&mut operation) };
        if code != 0 {
            bail!("the shell could not move the file to the Recycle Bin (code {code})");
        }
        if operation.fAnyOperationsAborted != 0 {
            bail!("moving the file to the Recycle Bin was cancelled");
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("the Recycle Bin is only available on Windows")
    }
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        bail!("the path contains an interior NUL character");
    }
    wide.push(0);
    wide.push(0);
    Ok(wide)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[cfg(windows)]
    #[test]
    fn shell_paths_are_double_nul_terminated() {
        let encoded = wide_path(&PathBuf::from(r"C:\Photos\holiday.jpg")).unwrap();
        assert_eq!(&encoded[encoded.len() - 2..], &[0, 0]);
        assert_eq!(encoded.iter().filter(|unit| **unit == 0).count(), 2);
    }

    #[test]
    fn missing_files_report_a_shell_error() {
        let path =
            std::env::temp_dir().join(format!("picasa-rs-missing-{}.jpg", std::process::id()));
        let error = move_to_recycle_bin(&path).unwrap_err();
        assert!(
            format!("{error:#}").contains("Recycle Bin")
                || format!("{error:#}").contains("cancelled"),
            "unexpected error: {error:#}"
        );
    }
}
