//! Inicio con Windows: un valor en HKCU\...\Run (solo tu usuario, sin permisos de admin).

use std::ptr;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Pega";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn is_enabled() -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        ) == ERROR_SUCCESS
    }
}

/// Arranca oculto (`--hidden`): solo queda el ícono en la bandeja.
pub fn set(enabled: bool) -> bool {
    let (key, value) = (wide(RUN_KEY), wide(VALUE));
    unsafe {
        if !enabled {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr());
            return r == ERROR_SUCCESS || !is_enabled();
        }
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let command = wide(&format!("\"{}\" --hidden", exe.display()));
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * 2) as u32,
        ) == ERROR_SUCCESS
    }
}

/// Si el .exe cambió de carpeta, la entrada apunta al lugar nuevo.
pub fn refresh() {
    if is_enabled() {
        set(true);
    }
}
