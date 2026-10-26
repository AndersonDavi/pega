//! Todo lo que habla con Win32: escuchar el portapapeles por eventos (sin sondeo),
//! el atajo global, leer/escribir texto y simular Ctrl+V en la ventana anterior.

use crate::image::{self, Rgba};
use crate::state::{self, MAX_TEXT_BYTES, State};
use egui::ViewportCommand;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use std::{mem, ptr, thread};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, RegisterHotKey, SendInput, VK_CONTROL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumThreadWindows, FindWindowExW,
    GetForegroundWindow, GetMessageW, GetWindowTextW, HWND_MESSAGE, IsWindow, IsWindowVisible, MSG,
    PostMessageW, RegisterClassW, SW_SHOW, SetForegroundWindow, ShowWindow, TranslateMessage,
    WM_APP, WM_CLIPBOARDUPDATE, WM_HOTKEY, WNDCLASSW,
};

pub const HOTKEY_LABEL: &str = "Ctrl+Alt+V";
pub const WINDOW_TITLE: &str = "Pega";
const VK_V: u16 = 0x56;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const HOTKEY_ID: i32 = 1;
/// Otra instancia de Pega pide que esta se muestre.
const WM_SHOW: u32 = WM_APP + 1;
const CLASS: &str = "PegaListener";

/// Ventana de egui.
static APP_HWND: AtomicIsize = AtomicIsize::new(0);
/// Ventana oculta (solo mensajes) que escucha el portapapeles y el atajo.
static LISTENER_HWND: AtomicIsize = AtomicIsize::new(0);
/// Ventana que tenía el foco antes de abrir Pega: ahí se pega.
static PREV_FG: AtomicIsize = AtomicIsize::new(0);
/// La ventana se acaba de mostrar: la interfaz enfoca la búsqueda.
static SHOWN: AtomicBool = AtomicBool::new(false);

struct Shared {
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
}
static SHARED: OnceLock<Shared> = OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn format_id(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
}

pub fn take_shown() -> bool {
    SHOWN.swap(false, Relaxed)
}

/// true si ya hay otra instancia corriendo (el mutex vive hasta que el proceso termina).
pub fn already_running() -> bool {
    unsafe {
        CreateMutexW(ptr::null(), 0, wide("Local\\PegaSingleInstance").as_ptr());
        GetLastError() == ERROR_ALREADY_EXISTS
    }
}

/// Le pide a la instancia que ya corre que se muestre.
pub fn signal_existing() {
    unsafe {
        let hwnd = FindWindowExW(
            HWND_MESSAGE,
            ptr::null_mut(),
            wide(CLASS).as_ptr(),
            ptr::null(),
        );
        if !hwnd.is_null() {
            PostMessageW(hwnd, WM_SHOW, 0, 0);
        }
    }
}

/// Busca la ventana de egui entre las del hilo actual (el runner no expone su HWND).
fn find_app_window() -> isize {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut buf = [0u16; 64];
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) } as usize;
        if String::from_utf16_lossy(&buf[..n]) == WINDOW_TITLE {
            unsafe { *(found as *mut isize) = hwnd as isize };
            return 0;
        }
        1
    }
    let mut found: isize = 0;
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(each),
            &mut found as *mut isize as LPARAM,
        )
    };
    found
}

/// Arranca el hilo Win32. Se llama desde el hilo de la interfaz, con la ventana ya creada.
pub fn start(state: Arc<Mutex<State>>, ctx: egui::Context) {
    APP_HWND.store(find_app_window(), Relaxed);
    let _ = SHARED.set(Shared { state, ctx });
    thread::Builder::new()
        .name("pega-win32".into())
        .spawn(|| unsafe { run() })
        .expect("hilo win32");
}

unsafe fn run() {
    unsafe {
        let class = wide(CLASS);
        let hinstance = GetModuleHandleW(ptr::null());
        let mut wc: WNDCLASSW = mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinstance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        LISTENER_HWND.store(hwnd as isize, Relaxed);
        AddClipboardFormatListener(hwnd);
        let hotkey_ok = RegisterHotKey(
            hwnd,
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            VK_V as u32,
        ) != 0;
        if let Some(s) = SHARED.get() {
            s.state.lock().unwrap().hotkey_ok = hotkey_ok;
            s.ctx.request_repaint();
        }
        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(shared) = SHARED.get() else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };
    match msg {
        WM_CLIPBOARDUPDATE => {
            let changed = match unsafe { read_clipboard(hwnd) } {
                Some(Captured::Text(text)) => shared.state.lock().unwrap().capture(text),
                Some(Captured::Dib(dib)) => capture_image(&shared.state, &dib),
                None => false,
            };
            if changed {
                shared.ctx.request_repaint();
            }
            0
        }
        WM_HOTKEY if wparam as i32 == HOTKEY_ID => {
            unsafe { toggle(&shared.ctx) };
            0
        }
        WM_SHOW => {
            unsafe { show(&shared.ctx) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn toggle(ctx: &egui::Context) {
    unsafe {
        let app = APP_HWND.load(Relaxed) as HWND;
        if GetForegroundWindow() == app && IsWindowVisible(app) != 0 {
            hide(ctx);
        } else {
            show(ctx);
        }
    }
}

unsafe fn show(ctx: &egui::Context) {
    unsafe {
        let app = APP_HWND.load(Relaxed) as HWND;
        let fg = GetForegroundWindow();
        if fg != app {
            PREV_FG.store(fg as isize, Relaxed);
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        // El atajo le da a este proceso permiso de tomar el foco; se usa aquí mismo.
        ShowWindow(app, SW_SHOW);
        SetForegroundWindow(app);
        SHOWN.store(true, Relaxed);
        ctx.request_repaint();
    }
}

pub fn hide(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    ctx.request_repaint();
}

/// Abre el portapapeles con reintentos: otra app puede tenerlo abierto un instante.
unsafe fn open_clipboard(owner: HWND) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(owner) } != 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

enum Captured {
    Text(String),
    /// Bytes crudos de CF_DIB; se decodifican ya con el portapapeles cerrado.
    Dib(Vec<u8>),
}

/// Las capturas (ShareX, Win+Shift+S…) suelen avisar dos o más veces con la misma
/// imagen; el hash de píxeles hace que solo suba al principio en vez de duplicarse.
fn capture_image(state: &Mutex<State>, dib: &[u8]) -> bool {
    let Some(img) = image::from_dib(dib) else {
        return false;
    };
    let hash = image::hash(&img);
    if let Some(changed) = state.lock().unwrap().bump_image(hash) {
        return changed;
    }
    match state::prepare_image(&img, hash) {
        Some(prepared) => state.lock().unwrap().insert_image(prepared),
        None => false,
    }
}

unsafe fn read_clipboard(owner: HWND) -> Option<Captured> {
    unsafe {
        // Convención de Windows: los gestores de contraseñas marcan así lo que no se debe guardar.
        for name in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "Clipboard Viewer Ignore",
        ] {
            if IsClipboardFormatAvailable(format_id(name)) != 0 {
                return None;
            }
        }
        // Si hay texto se guarda el texto (Excel, por ejemplo, también pone una imagen).
        let is_text = IsClipboardFormatAvailable(CF_UNICODETEXT) != 0;
        if !is_text && IsClipboardFormatAvailable(CF_DIB) == 0 || !open_clipboard(owner) {
            return None;
        }
        let _guard = ClipboardGuard;
        let can_include = format_id("CanIncludeInClipboardHistory");
        if IsClipboardFormatAvailable(can_include) != 0 {
            let h = GetClipboardData(can_include);
            let p = GlobalLock(h) as *const u32;
            if !p.is_null() {
                let allowed = *p != 0;
                GlobalUnlock(h);
                if !allowed {
                    return None;
                }
            }
        }
        if !is_text {
            let h = GetClipboardData(CF_DIB);
            let p = GlobalLock(h) as *const u8;
            if p.is_null() {
                return None;
            }
            let dib = std::slice::from_raw_parts(p, GlobalSize(h)).to_vec();
            GlobalUnlock(h);
            return Some(Captured::Dib(dib));
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let p = GlobalLock(h) as *const u16;
        if p.is_null() {
            return None;
        }
        let max = GlobalSize(h) / 2;
        let text = if max > MAX_TEXT_BYTES {
            None
        } else {
            let len = (0..max).find(|&i| *p.add(i) == 0).unwrap_or(max);
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
        };
        GlobalUnlock(h);
        text.map(Captured::Text)
    }
}

unsafe fn set_data(format: u32, bytes: &[u8]) {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if h.is_null() {
            return;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return;
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        // Si SetClipboardData acepta el bloque, Windows pasa a ser su dueño.
        if SetClipboardData(format, h as *mut c_void).is_null() {
            GlobalFree(h);
        }
    }
}

/// `private`: se marca para que ni Pega, ni el historial de Windows (Win+V), ni la nube lo guarden.
pub fn write_text(text: &str, private: bool) -> bool {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if !open_clipboard(LISTENER_HWND.load(Relaxed) as HWND) {
            return false;
        }
        let _guard = ClipboardGuard;
        EmptyClipboard();
        set_data(
            CF_UNICODETEXT,
            std::slice::from_raw_parts(units.as_ptr() as *const u8, units.len() * 2),
        );
        if private {
            set_data(
                format_id("ExcludeClipboardContentFromMonitorProcessing"),
                &0u32.to_le_bytes(),
            );
            set_data(
                format_id("CanIncludeInClipboardHistory"),
                &0u32.to_le_bytes(),
            );
            set_data(format_id("CanUploadToCloudClipboard"), &0u32.to_le_bytes());
        }
    }
    true
}

/// Pone la imagen como bitmap (lo entiende todo) y como PNG (navegadores, Office, Discord).
pub fn write_image(img: &Rgba, png: &[u8]) -> bool {
    unsafe {
        if !open_clipboard(LISTENER_HWND.load(Relaxed) as HWND) {
            return false;
        }
        let _guard = ClipboardGuard;
        EmptyClipboard();
        set_data(CF_DIB, &image::to_dib(img));
        set_data(format_id("PNG"), png);
    }
    true
}

/// Tras copiar: oculta Pega, devuelve el foco a la ventana anterior y manda Ctrl+V.
pub fn paste_into_previous(ctx: &egui::Context) {
    hide(ctx);
    let prev = PREV_FG.load(Relaxed);
    thread::spawn(move || unsafe {
        let prev = prev as HWND;
        // Sin una ventana destino conocida solo se copia: no se pega a ciegas.
        if prev.is_null() || IsWindow(prev) == 0 {
            return;
        }
        thread::sleep(Duration::from_millis(60));
        SetForegroundWindow(prev);
        thread::sleep(Duration::from_millis(40));
        send_ctrl_v();
    });
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

unsafe fn send_ctrl_v() {
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<INPUT>() as i32,
        )
    };
}
