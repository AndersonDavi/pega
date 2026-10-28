//! Acceso directo en el menú Inicio, para que Pega salga al buscarlo en Windows (un .exe
//! suelto no aparece). El .lnk se arma a mano con el formato MS-SHLLINK, solo la ruta del
//! .exe y su carpeta: sin COM ni IShellLink, que cargarían media Shell en el proceso.

use crate::store::Store;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::UI::Shell::{ILFree, ILGetSize, SHSimpleIDListFromPath};

/// En la base: el acceso ya se creó una vez. Si después falta, lo borraste y no vuelve.
const META: &str = "start_menu_link";

const HEADER_SIZE: u32 = 0x4C;
/// CLSID de los accesos directos: 00021401-0000-0000-C000-000000000046.
const LINK_CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];
const HAS_TARGET_ID_LIST: u32 = 0x01;
const HAS_LINK_INFO: u32 = 0x02;
const HAS_WORKING_DIR: u32 = 0x10;
const IS_UNICODE: u32 = 0x80;
const SW_SHOWNORMAL: u32 = 1;
/// LinkInfo con las rutas en UTF-16 además de ANSI (cabecera de 0x24 bytes).
const LINK_INFO_HEADER: u32 = 0x24;
const VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 1;
const DRIVE_FIXED: u32 = 3;

fn link_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs\Pega.lnk"))
}

/// Crea el acceso o lo corrige si el .exe cambió de carpeta. Nada en modo portable ni si
/// Pega corre desde la carpeta temporal (abierto dentro del zip, o una compilación).
pub fn refresh(store: &Store) {
    let (Ok(exe), Some(link)) = (std::env::current_exe(), link_path()) else {
        return;
    };
    let Some(dir) = exe.parent() else { return };
    if crate::paths::portable_dir(dir).is_some() || is_temporary(&exe) {
        return;
    }
    let made = store.get_meta(META).ok().flatten().is_some();
    if made && !link.exists() {
        return;
    }
    let bytes = build(&exe);
    let same = std::fs::read(&link).is_ok_and(|old| old == bytes);
    if (same || std::fs::write(&link, &bytes).is_ok()) && !made {
        let _ = store.set_meta(META, b"1");
    }
}

fn is_temporary(exe: &Path) -> bool {
    // Ruta larga (TEMP puede venir en formato 8.3), sin el prefijo \\?\ de canonicalize.
    let full = |p: &Path| {
        let path = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let text = path.to_string_lossy().to_lowercase();
        text.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(text)
    };
    full(exe).starts_with(&full(&std::env::temp_dir()))
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().collect()
}

/// La ruta como lista de ID de la Shell: sin ella, Windows abre el acceso pero no sabe a qué
/// apunta (no lo muestra como programa). Shell32 ya está cargada (el ícono de la bandeja).
fn id_list(target: &[u16]) -> Option<Vec<u8>> {
    let path: Vec<u16> = target.iter().copied().chain([0]).collect();
    unsafe {
        let pidl = SHSimpleIDListFromPath(path.as_ptr());
        if pidl.is_null() {
            return None;
        }
        // El tamaño incluye los 2 bytes en cero que cierran la lista.
        let len = ILGetSize(pidl) as usize;
        let bytes = std::slice::from_raw_parts(pidl.cast::<u8>(), len).to_vec();
        ILFree(pidl);
        (len <= u16::MAX as usize).then_some(bytes)
    }
}

/// El archivo .lnk que abre `exe` en su carpeta.
pub fn build(exe: &Path) -> Vec<u8> {
    let target = wide(exe);
    let dir = exe.parent().map(wide).unwrap_or_default();
    let ids = id_list(&target);
    let mut flags = HAS_LINK_INFO | HAS_WORKING_DIR | IS_UNICODE;
    if ids.is_some() {
        flags |= HAS_TARGET_ID_LIST;
    }
    let mut out = Vec::with_capacity(1024 + 4 * target.len());
    out.extend(HEADER_SIZE.to_le_bytes());
    out.extend(LINK_CLSID);
    out.extend(flags.to_le_bytes());
    // Atributos, tres fechas, tamaño e índice del ícono: en cero, Windows los lee del .exe.
    out.extend([0; 4 + 24 + 4 + 4]);
    out.extend(SW_SHOWNORMAL.to_le_bytes());
    out.extend([0; 2 + 10]); // tecla rápida y reservados
    if let Some(ids) = ids {
        out.extend((ids.len() as u16).to_le_bytes());
        out.extend(ids);
    }
    out.extend(link_info(&target));
    out.extend((dir.len() as u16).to_le_bytes());
    out.extend(dir.iter().flat_map(|c| c.to_le_bytes()));
    out.extend(0u32.to_le_bytes()); // fin de los bloques extra
    out
}

/// Dónde está el destino: un volumen local y la ruta completa.
fn link_info(target: &[u16]) -> Vec<u8> {
    // La versión ANSI solo la leería un Windows viejo; la que vale es la UTF-16.
    let ansi: Vec<u8> = target
        .iter()
        .map(|&c| if c < 0x80 { c as u8 } else { b'?' })
        .chain([0])
        .collect();
    let volume_len = 17u32; // VolumeID de 16 bytes y una etiqueta vacía
    let base = LINK_INFO_HEADER + volume_len;
    let suffix = base + ansi.len() as u32;
    let base_w = suffix + 1;
    let suffix_w = base_w + 2 * (target.len() as u32 + 1);
    let size = suffix_w + 2;

    let mut out = Vec::with_capacity(size as usize);
    for v in [
        size,
        LINK_INFO_HEADER,
        VOLUME_ID_AND_LOCAL_BASE_PATH,
        LINK_INFO_HEADER, // VolumeID justo después de la cabecera
        base,
        0, // sin ruta de red
        suffix,
        base_w,
        suffix_w,
    ] {
        out.extend(v.to_le_bytes());
    }
    for v in [volume_len, DRIVE_FIXED, 0, 16] {
        out.extend(v.to_le_bytes());
    }
    out.push(0); // etiqueta del volumen
    out.extend(ansi);
    out.push(0); // sufijo ANSI vacío
    out.extend(target.iter().chain([&0]).flat_map(|c| c.to_le_bytes()));
    out.extend([0, 0]); // sufijo UTF-16 vacío
    debug_assert_eq!(out.len(), size as usize);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    fn utf16_at(b: &[u8], at: usize) -> String {
        let units: Vec<u16> = b[at..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn link_points_to_the_exe_and_its_folder() {
        let exe = Path::new(r"C:\Users\Andrés\Programas\Pega\pega.exe");
        let b = build(exe);
        assert_eq!(u32_at(&b, 0), HEADER_SIZE);
        assert_eq!(b[4..20], LINK_CLSID);
        assert_eq!(
            u32_at(&b, 20),
            HAS_TARGET_ID_LIST | HAS_LINK_INFO | HAS_WORKING_DIR | IS_UNICODE
        );
        assert_eq!(u32_at(&b, 60), SW_SHOWNORMAL);

        // La lista de ID (largo + bytes, cerrada con dos ceros) y después LinkInfo.
        let ids = u16::from_le_bytes([b[76], b[77]]) as usize;
        assert!(ids > 2);
        assert_eq!(b[78 + ids - 2..78 + ids], [0, 0]);
        let start = 78 + ids;
        let info = &b[start..];
        let size = u32_at(info, 0) as usize;
        assert_eq!(u32_at(info, 4), LINK_INFO_HEADER);
        assert_eq!(u32_at(info, 8), VOLUME_ID_AND_LOCAL_BASE_PATH);
        assert_eq!(u32_at(info, 12), LINK_INFO_HEADER);
        // La ruta completa, con tilde, en la versión UTF-16; la ANSI la cambia por "?".
        assert_eq!(utf16_at(info, u32_at(info, 28) as usize), exe.to_str().unwrap());
        let ansi_at = u32_at(info, 16) as usize;
        assert!(info[ansi_at..].starts_with(br"C:\Users\Andr?s\"));

        // Después de LinkInfo: la carpeta de trabajo (largo + UTF-16) y el bloque final.
        let rest = &b[start + size..];
        let len = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        let dir: Vec<u16> = rest[2..2 + 2 * len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(String::from_utf16(&dir).unwrap(), r"C:\Users\Andrés\Programas\Pega");
        assert_eq!(&rest[2 + 2 * len..], [0, 0, 0, 0]);
    }

    #[test]
    fn temporary_folder_is_detected() {
        assert!(is_temporary(&std::env::temp_dir().join("pega.exe")));
        assert!(!is_temporary(Path::new(r"C:\Programas\Pega\pega.exe")));
    }
}
