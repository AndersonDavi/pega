//! Cifrado en reposo con DPAPI: la clave la guarda Windows atada a tu usuario,
//! así que otro usuario (o el archivo copiado a otro PC) no puede leer los datos.

use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// Entropía propia de la app: sin ella, CryptUnprotectData falla aunque sea el mismo usuario.
const ENTROPY: &[u8] = b"pega/v1";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Copia el resultado de DPAPI a un Vec y libera el buffer que reservó Windows.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        v
    }
}

pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    let input = blob(plain);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    unsafe {
        let ok = CryptProtectData(
            &input,
            null(),
            &entropy,
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        (ok != 0).then(|| take(out))
    }
}

pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
    let input = blob(cipher);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    unsafe {
        let ok = CryptUnprotectData(
            &input,
            null_mut(),
            &entropy,
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        (ok != 0).then(|| take(out))
    }
}

pub fn protect_str(s: &str) -> Option<Vec<u8>> {
    protect(s.as_bytes())
}

pub fn unprotect_str(cipher: &[u8]) -> Option<String> {
    unprotect(cipher).and_then(|v| String::from_utf8(v).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_no_plaintext() {
        let secret = "Cédula 1098765432";
        let enc = protect_str(secret).unwrap();
        assert!(
            !enc.windows(10).any(|w| w == b"1098765432"),
            "no queda en claro"
        );
        assert_eq!(unprotect_str(&enc).unwrap(), secret);
        // Cada cifrado es distinto (DPAPI usa una sal nueva), pero ambos se descifran.
        assert_ne!(enc, protect_str(secret).unwrap());
    }

    #[test]
    fn tampered_or_foreign_data_fails() {
        let mut enc = protect(b"hola").unwrap();
        let last = enc.len() - 1;
        enc[last] ^= 0xFF;
        assert!(unprotect(&enc).is_none());
        assert!(unprotect(b"basura que no es DPAPI").is_none());
    }
}
