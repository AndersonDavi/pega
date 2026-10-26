//! Dónde viven los datos.

use std::path::{Path, PathBuf};

/// Modo portable: con un archivo `portable` junto al .exe, los datos van a `datos\` al lado
/// (como Ditto). Siguen cifrados con DPAPI: para otro usuario o PC, usar la exportación.
pub fn portable_dir(exe_dir: &Path) -> Option<PathBuf> {
    exe_dir
        .join("portable")
        .exists()
        .then(|| exe_dir.join("datos"))
}

/// `%APPDATA%\Pega`, o la carpeta portable si corresponde.
pub fn data_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir.as_deref().and_then(portable_dir) {
        return dir;
    }
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Pega")
}

pub fn database() -> PathBuf {
    data_dir().join("pega.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_mode_uses_folder_next_to_exe() {
        let dir = std::env::temp_dir().join(format!("pega-portable-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(portable_dir(&dir), None);
        std::fs::write(dir.join("portable"), b"").unwrap();
        assert_eq!(portable_dir(&dir), Some(dir.join("datos")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
