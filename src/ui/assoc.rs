//! "Open with" / default-app registration (Windows). Per-user registry keys, so no administrator rights.
//! Windows does not let a program make itself the default: this adds Parquet Viewer to the "Open with"
//! list and to Settings > Default apps, and the user then chooses it there.

#[cfg(windows)]
mod win {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const PROG_ID: &str = "ParquetViewer.File";
    const APP_KEY: &str = "ParquetViewer"; // name under RegisteredApplications
    const EXTS: [&str; 2] = [".parquet", ".pq"];
    const CLASSES: &str = r"HKCU\Software\Classes";
    const CAPS: &str = r"HKCU\Software\ParquetViewer\Capabilities";
    const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW: no console flash

    fn run(mut c: Command) -> Result<(), String> {
        c.creation_flags(NO_WINDOW);
        let out = c.output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
    }
    /// reg add <key> [/v name | /ve] /t REG_SZ /d value /f
    fn set(key: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let mut c = Command::new("reg");
        c.args(["add", key]);
        match name { Some(n) => { c.args(["/v", n]); } None => { c.arg("/ve"); } }
        c.args(["/t", "REG_SZ", "/d", value, "/f"]);
        run(c)
    }
    /// An empty marker value (how "Open with" lists are stored).
    fn mark(key: &str, name: &str) -> Result<(), String> {
        let mut c = Command::new("reg");
        c.args(["add", key, "/v", name, "/t", "REG_NONE", "/f"]);
        run(c)
    }
    /// Deleting something that is not there is fine.
    fn remove(key: &str, value: Option<&str>) {
        let mut c = Command::new("reg");
        c.args(["delete", key]);
        if let Some(v) = value { c.args(["/v", v]); }
        c.arg("/f");
        let _ = run(c);
    }

    pub fn register() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe_name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "parquet-viewer.exe".into());
        let exe = exe.to_string_lossy().to_string();
        let open_cmd = format!("\"{exe}\" \"%1\"");

        // the file type
        set(&format!(r"{CLASSES}\{PROG_ID}"), None, "Parquet file")?;
        set(&format!(r"{CLASSES}\{PROG_ID}\DefaultIcon"), None, &format!("\"{exe}\",0"))?;
        set(&format!(r"{CLASSES}\{PROG_ID}\shell\open\command"), None, &open_cmd)?;
        // listed in "Open with" for .parquet and .pq (does not change the current default)
        for ext in EXTS { mark(&format!(r"{CLASSES}\{ext}\OpenWithProgids"), PROG_ID)?; }
        // the program itself
        let app = format!(r"{CLASSES}\Applications\{exe_name}");
        set(&format!(r"{app}\shell\open\command"), None, &open_cmd)?;
        set(&app, Some("FriendlyAppName"), "Parquet Viewer")?;
        for ext in EXTS { set(&format!(r"{app}\SupportedTypes"), Some(ext), "")?; }
        // appears in Settings > Default apps
        set(CAPS, Some("ApplicationName"), "Parquet Viewer")?;
        set(CAPS, Some("ApplicationDescription"), "Fast viewer and search tool for large Parquet files")?;
        for ext in EXTS { set(&format!(r"{CAPS}\FileAssociations"), Some(ext), PROG_ID)?; }
        set(r"HKCU\Software\RegisteredApplications", Some(APP_KEY), r"Software\ParquetViewer\Capabilities")?;
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        let exe_name = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "parquet-viewer.exe".into());
        for ext in EXTS { remove(&format!(r"{CLASSES}\{ext}\OpenWithProgids"), Some(PROG_ID)); }
        remove(&format!(r"{CLASSES}\{PROG_ID}"), None);
        remove(&format!(r"{CLASSES}\Applications\{exe_name}"), None);
        remove(r"HKCU\Software\RegisteredApplications", Some(APP_KEY));
        remove(r"HKCU\Software\ParquetViewer", None);
        Ok(())
    }

    /// Opens the Windows page where the user picks default apps (on newer Windows, the page for this app).
    pub fn open_default_apps_settings() {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", "ms-settings:defaultapps?registeredAppUser=ParquetViewer"]);
        c.creation_flags(NO_WINDOW);
        let _ = c.spawn();
    }
}

#[cfg(windows)]
pub(crate) use win::{open_default_apps_settings, register, unregister};

#[cfg(not(windows))]
pub(crate) fn register() -> Result<(), String> { Err("only available on Windows".into()) }
#[cfg(not(windows))]
pub(crate) fn unregister() -> Result<(), String> { Err("only available on Windows".into()) }
#[cfg(not(windows))]
pub(crate) fn open_default_apps_settings() {}
