use crate::{err, Reply};
use omascribe_core::platform::child_command;
use std::path::Path;

pub fn open(path: &Path, select: bool) -> Reply<()> {
    #[cfg(target_os = "macos")]
    {
        let mut command = child_command("open");
        if select {
            command.arg("-R");
        }
        let status = command.arg(path).status().map_err(err)?;
        if !status.success() {
            return Err("Finder could not open this location".into());
        }
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = child_command("explorer.exe");
        if select {
            command.arg(format!("/select,{}", path.display()));
        } else {
            command.arg(path);
        }
        command.spawn().map_err(err)?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        if select {
            let uri = tauri::Url::from_file_path(path).map_err(|_| "Invalid file path")?;
            if child_command("gdbus")
                .args([
                    "call",
                    "--session",
                    "--dest",
                    "org.freedesktop.FileManager1",
                    "--object-path",
                    "/org/freedesktop/FileManager1",
                    "--method",
                    "org.freedesktop.FileManager1.ShowItems",
                    &format!("['{uri}']"),
                    "",
                ])
                .output()
                .is_ok_and(|o| o.status.success())
            {
                return Ok(());
            }
        }
        let directory = if select {
            path.parent().ok_or("Missing parent folder")?
        } else {
            path
        };
        child_command("xdg-open")
            .arg(directory)
            .spawn()
            .map_err(err)?;
        Ok(())
    }
}
