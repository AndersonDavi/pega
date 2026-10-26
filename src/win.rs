//! Todo lo que habla con Win32: escuchar el portapapeles por eventos (sin sondeo),
//! el atajo global, leer/escribir texto y simular Ctrl+V en la ventana anterior.

use crate::autostart;
use crate::i18n::t;
use crate::image::{self, Rgba};
use crate::settings::Hotkey;
use crate::state::{self, MAX_TEXT_BYTES, State};
use crate::tr;
use egui::ViewportCommand;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use std::{mem, ptr, thread};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, HeapOptimizeResources,
    HeapSetInformation,
};
use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MOD_ALT,
    MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, SendInput, UnregisterHotKey,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIN_BALLOONUSERCLICK, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    EnumThreadWindows, FindWindowW, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId, HWND_MESSAGE, IMAGE_ICON, IsWindow,
    IsWindowVisible, KillTimer, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW, MF_CHECKED, MF_SEPARATOR,
    MF_STRING, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON,
    SW_SHOW, SetForegroundWindow, SetTimer, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLIPBOARDUPDATE, WM_HOTKEY,
    WM_LBUTTONUP, WM_NULL, WM_POWERBROADCAST, WM_RBUTTONUP, WM_TIMECHANGE, WM_TIMER, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

pub const WINDOW_TITLE: &str = "Pega";
const VK_V: u16 = 0x56;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const HOTKEY_ID: i32 = 1;
/// Atajo de "pegar sin formato".
const PLAIN_HOTKEY_ID: i32 = 2;
/// Otra instancia de Pega pide que esta se muestre.
const WM_SHOW: u32 = WM_APP + 1;
/// Clics sobre el ícono de la bandeja.
const WM_TRAY: u32 = WM_APP + 2;
/// Los ajustes cambiaron el atajo.
const WM_REHOTKEY: u32 = WM_APP + 3;
/// Cambió algo que afecta a lo temporal (ajustes, fijar, borrar): reprogramar el temporizador.
const WM_RESCHEDULE: u32 = WM_APP + 4;
/// `pega-cli` cambió la base: volver a leerla.
const WM_RELOAD: u32 = WM_APP + 5;
/// Temporizador de las entradas que se borran solas.
const TIMER_EXPIRE: usize = 1;
const CLASS: &str = "PegaListener";
const TRAY_ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_AUTOSTART: usize = 2;
const MENU_QUIT: usize = 3;

/// Ventana de egui.
static APP_HWND: AtomicIsize = AtomicIsize::new(0);
/// Ventana oculta que escucha el portapapeles, el atajo y la bandeja. Es de nivel superior
/// (nunca se muestra) y no "solo mensajes", para recibir TaskbarCreated si explorer se reinicia.
static LISTENER_HWND: AtomicIsize = AtomicIsize::new(0);
static TRAY_OK: AtomicBool = AtomicBool::new(false);
static TASKBAR_CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Ventana que tenía el foco antes de abrir Pega: ahí se pega.
static PREV_FG: AtomicIsize = AtomicIsize::new(0);
/// La ventana se acaba de mostrar: la interfaz enfoca la búsqueda.
static SHOWN: AtomicBool = AtomicBool::new(false);
/// La ventana se está ocultando: la interfaz suelta lo que no necesita mientras no se ve.
static HIDING: AtomicBool = AtomicBool::new(false);

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

pub fn take_hiding() -> bool {
    HIDING.swap(false, Relaxed)
}

/// Con la ventana ya oculta, devuelve a Windows las páginas de RAM física que no se usan.
/// Al volver a mostrarla se recuperan las que hagan falta (unos milisegundos).
pub fn trim_memory() {
    trim_memory_after(Duration::from_millis(300));
}

/// Igual, tras `delay` (al arrancar oculto, cuando ya cargó todo).
pub fn trim_memory_after(delay: Duration) {
    thread::spawn(move || {
        thread::sleep(delay);
        unsafe {
            // Que el heap devuelva a Windows los bloques libres que guarda en caché
            // (EmptyWorkingSet solo saca páginas de la RAM física, no libera memoria).
            #[repr(C)]
            struct OptimizeResources {
                version: u32,
                flags: u32,
            }
            let info = OptimizeResources {
                version: 1,
                flags: 0,
            };
            HeapSetInformation(
                ptr::null_mut(),
                HeapOptimizeResources,
                (&info as *const OptimizeResources).cast(),
                std::mem::size_of::<OptimizeResources>(),
            );
            K32EmptyWorkingSet(GetCurrentProcess());
        }
    });
}

/// Error que impide arrancar: sin consola, se muestra en un cuadro de diálogo y se sale.
pub fn fatal(msg: &str) -> ! {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            wide(msg).as_ptr(),
            wide(WINDOW_TITLE).as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
    std::process::exit(1)
}

/// true si ya hay otra instancia corriendo (el mutex vive hasta que el proceso termina).
pub fn already_running() -> bool {
    unsafe {
        let h = CreateMutexW(ptr::null(), 0, wide("Local\\PegaSingleInstance").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return true;
        }
        false
    }
}

/// Al reiniciar (restaurar un respaldo), espera a que la instancia anterior termine.
pub fn wait_for_previous_instance() -> bool {
    for _ in 0..50 {
        if !already_running() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

/// HWND de la ventana principal, para que los diálogos de archivo sean modales.
pub fn app_hwnd() -> isize {
    APP_HWND.load(Relaxed)
}

/// Le pide a la instancia que ya corre que se muestre.
pub fn signal_existing() {
    post_to_running(WM_SHOW);
}

/// `pega-cli` cambió la base: si la app está abierta, que la vuelva a leer.
pub fn notify_reload() {
    post_to_running(WM_RELOAD);
}

fn post_to_running(msg: u32) {
    unsafe {
        let hwnd = FindWindowW(wide(CLASS).as_ptr(), ptr::null());
        if !hwnd.is_null() {
            PostMessageW(hwnd, msg, 0, 0);
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
    let repaint = ctx.clone();
    crate::sync::start(state.clone(), move || {
        repaint.request_repaint();
        reschedule_expiry();
        // Con la ventana oculta, lo que usó la mezcla vuelve a Windows.
        let app = app_hwnd() as HWND;
        if !app.is_null() && unsafe { IsWindowVisible(app) } == 0 {
            trim_memory();
        }
    });
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
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        LISTENER_HWND.store(hwnd as isize, Relaxed);
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            Relaxed,
        );
        add_tray(hwnd);
        AddClipboardFormatListener(hwnd);
        apply_hotkey(hwnd);
        schedule_expiry(hwnd);
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
            let app = source_app();
            let changed = match unsafe { read_clipboard(hwnd) } {
                Some(Captured::Text(text)) => shared.state.lock().unwrap().capture(text, app),
                Some(Captured::Dib(dib)) => capture_image(&shared.state, &dib, app),
                None => false,
            };
            if changed {
                shared.ctx.request_repaint();
                unsafe { schedule_expiry(hwnd) };
                crate::sync::poke();
            }
            0
        }
        WM_TIMER if wparam == TIMER_EXPIRE => {
            if shared.state.lock().unwrap().purge_expired() {
                shared.ctx.request_repaint();
                crate::sync::poke();
            }
            unsafe { schedule_expiry(hwnd) };
            0
        }
        // Cambió la hora o el PC volvió de suspensión: el temporizador es relativo, así que
        // los códigos y capturas sensibles se reprograman con el reloj nuevo.
        WM_RESCHEDULE | WM_TIMECHANGE | WM_POWERBROADCAST => {
            unsafe { schedule_expiry(hwnd) };
            if msg == WM_POWERBROADCAST {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            0
        }
        WM_RELOAD => {
            shared.state.lock().unwrap().reload();
            unsafe { schedule_expiry(hwnd) };
            crate::sync::poke();
            shared.ctx.request_repaint();
            0
        }
        WM_HOTKEY if wparam as i32 == HOTKEY_ID => {
            unsafe { toggle(&shared.ctx) };
            0
        }
        WM_HOTKEY if wparam as i32 == PLAIN_HOTKEY_ID => {
            paste_plain();
            0
        }
        WM_SHOW => {
            unsafe { show(&shared.ctx) };
            0
        }
        WM_REHOTKEY => {
            unsafe { apply_hotkey(hwnd) };
            0
        }
        WM_TRAY => {
            match lparam as u32 {
                WM_LBUTTONUP | NIN_BALLOONUSERCLICK => unsafe { show(&shared.ctx) },
                WM_RBUTTONUP => unsafe { tray_menu(hwnd, &shared.ctx) },
                _ => {}
            }
            0
        }
        // Explorer se reinició: la bandeja es nueva y hay que volver a poner el ícono.
        m if m != 0 && m == TASKBAR_CREATED.load(Relaxed) => {
            unsafe { add_tray(hwnd) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn hotkey_label() -> String {
    SHARED
        .get()
        .map(|s| s.state.lock().unwrap().settings.hotkey.label())
        .unwrap_or_default()
}

/// (Re)registra el atajo de los ajustes y actualiza el texto de la bandeja.
unsafe fn apply_hotkey(hwnd: HWND) {
    let Some(shared) = SHARED.get() else { return };
    let (hk, plain) = {
        let st = shared.state.lock().unwrap();
        (st.settings.hotkey, st.settings.plain_hotkey)
    };
    let register = |id: i32, hk: Hotkey| -> bool {
        let mut mods = MOD_NOREPEAT;
        for (on, m) in [
            (hk.ctrl, MOD_CONTROL),
            (hk.alt, MOD_ALT),
            (hk.shift, MOD_SHIFT),
            (hk.win, MOD_WIN),
        ] {
            if on {
                mods |= m;
            }
        }
        unsafe { RegisterHotKey(hwnd, id, mods, hk.vk as u32) != 0 }
    };
    unsafe {
        UnregisterHotKey(hwnd, HOTKEY_ID);
        UnregisterHotKey(hwnd, PLAIN_HOTKEY_ID);
    }
    let ok = register(HOTKEY_ID, hk);
    let plain_ok = plain.is_none_or(|p| register(PLAIN_HOTKEY_ID, p));
    {
        let mut st = shared.state.lock().unwrap();
        st.hotkey_ok = ok;
        st.plain_hotkey_ok = plain_ok;
    }
    if TRAY_OK.load(Relaxed) {
        let mut nid = tray_data(hwnd);
        nid.uFlags = NIF_TIP;
        set_tip(&mut nid, &hk.label());
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }
    shared.ctx.request_repaint();
}
/// Un único temporizador hasta el próximo vencimiento; ninguno si no hay nada temporal.
unsafe fn schedule_expiry(hwnd: HWND) {
    let Some(shared) = SHARED.get() else { return };
    let next = shared.state.lock().unwrap().next_expiry();
    unsafe {
        KillTimer(hwnd, TIMER_EXPIRE);
        if let Some(at) = next {
            // Entre 1 s y el máximo de Windows (~24 días).
            let delay = (at - state::now()).clamp(1_000, 0x7FFF_FFFF) as u32;
            SetTimer(hwnd, TIMER_EXPIRE, delay, None);
        }
    }
}

pub fn reschedule_expiry() {
    unsafe { PostMessageW(LISTENER_HWND.load(Relaxed) as HWND, WM_RESCHEDULE, 0, 0) };
}

/// La interfaz cambió el atajo en los ajustes.
pub fn reload_hotkey() {
    unsafe { PostMessageW(LISTENER_HWND.load(Relaxed) as HWND, WM_REHOTKEY, 0, 0) };
}

fn set_tip(nid: &mut NOTIFYICONDATAW, hotkey: &str) {
    let tip: Vec<u16> = format!("Pega — {hotkey}")
        .encode_utf16()
        .take(nid.szTip.len() - 1)
        .collect();
    nid.szTip = [0; 128];
    nid.szTip[..tip.len()].copy_from_slice(&tip);
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_ID;
    nid
}

unsafe fn add_tray(hwnd: HWND) {
    unsafe {
        let mut nid = tray_data(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        // Ícono 1 del .exe (pega.rc), al tamaño chico del sistema para que no se vea borroso.
        nid.hIcon = LoadImageW(
            GetModuleHandleW(ptr::null()),
            1 as *const u16,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            // Compartido: lo libera Windows; si no, cada reinicio de Explorer dejaba uno.
            LR_DEFAULTCOLOR | LR_SHARED,
        );
        set_tip(&mut nid, &hotkey_label());
        TRAY_OK.store(Shell_NotifyIconW(NIM_ADD, &nid) != 0, Relaxed);
    }
}

unsafe fn tray_menu(hwnd: HWND, ctx: &egui::Context) {
    unsafe {
        let menu = CreatePopupMenu();
        let autostart_flag = if autostart::is_enabled() {
            MF_CHECKED
        } else {
            0
        };
        let open = wide(&tr!("Abrir Pega\t{}", "Open Pega\t{}", hotkey_label()));
        AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(
            menu,
            MF_STRING | autostart_flag,
            MENU_AUTOSTART,
            wide(t("Iniciar con Windows", "Start with Windows")).as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        AppendMenuW(
            menu,
            MF_STRING,
            MENU_QUIT,
            wide(t("Salir", "Quit")).as_ptr(),
        );
        let mut pt = mem::zeroed();
        GetCursorPos(&mut pt);
        // Sin esto el menú no se cierra al hacer clic fuera (comportamiento documentado de Windows).
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            hwnd,
            ptr::null(),
        ) as usize;
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        match cmd {
            MENU_OPEN => show(ctx),
            MENU_AUTOSTART => {
                autostart::set(!autostart::is_enabled());
            }
            MENU_QUIT => quit(),
            _ => {}
        }
    }
}

/// Hay forma de volver a abrir la ventana si se oculta.
pub fn can_reopen(hotkey_ok: bool) -> bool {
    hotkey_ok || TRAY_OK.load(Relaxed)
}

/// Notificación de Windows desde el ícono de la bandeja (clic: abre Pega).
fn balloon(text: &str) {
    let hwnd = LISTENER_HWND.load(Relaxed) as HWND;
    if hwnd.is_null() || !TRAY_OK.load(Relaxed) {
        return;
    }
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_WARNING;
    for (dst, src) in [
        (&mut nid.szInfoTitle[..], WINDOW_TITLE),
        (&mut nid.szInfo[..], text),
    ] {
        let w: Vec<u16> = src.encode_utf16().take(dst.len() - 1).collect();
        dst[..w.len()].copy_from_slice(&w);
    }
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

pub fn remove_tray() {
    let hwnd = LISTENER_HWND.load(Relaxed) as HWND;
    if !hwnd.is_null() {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd)) };
    }
}

/// Quita el ícono de la bandeja y termina. SQLite en WAL no pierde nada ya escrito.
pub fn quit() -> ! {
    remove_tray();
    if let Some(shared) = SHARED.get() {
        crate::sync::write_final(&shared.state);
    }
    std::process::exit(0)
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
    HIDING.store(true, Relaxed);
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

/// Ejecutable que puso el contenido ("chrome.exe"): el dueño del portapapeles o, si
/// no declaró ventana, la que está al frente. None si es Pega misma.
fn source_app() -> Option<String> {
    unsafe {
        let mut hwnd = GetClipboardOwner();
        if hwnd.is_null() {
            hwnd = GetForegroundWindow();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(str::to_lowercase)
    }
}

enum Captured {
    Text(String),
    /// Bytes crudos de CF_DIB; se decodifican ya con el portapapeles cerrado.
    Dib(Vec<u8>),
}

/// Las capturas (ShareX, Win+Shift+S…) suelen avisar dos o más veces con la misma
/// imagen; el hash de píxeles hace que solo suba al principio en vez de duplicarse.
fn capture_image(state: &Mutex<State>, dib: &[u8], app: Option<String>) -> bool {
    if state.lock().unwrap().excluded(&app) {
        return false;
    }
    let Some(img) = image::from_dib(dib) else {
        return false;
    };
    let hash = image::hash(&img);
    if let Some(changed) = state.lock().unwrap().bump_image(hash) {
        return changed;
    }
    let Some(prepared) = state::prepare_image(&img, hash, app) else {
        return false;
    };
    let mut st = state.lock().unwrap();
    if !st.insert_image(prepared) {
        return false;
    }
    // OCR (si está activado) en otro hilo y otro proceso: la captura ya quedó guardada.
    let id = st
        .history
        .iter()
        .find(|e| e.image.as_ref().is_some_and(|i| i.hash == hash))
        .map(|e| e.id);
    let helper = crate::ocr::helper_path();
    if let (true, Some(id), Some(helper)) = (st.settings.ocr, id, helper) {
        drop(st);
        // Solo la versión reducida espera su turno; la imagen completa se suelta aquí.
        let data = crate::ocr::payload(&img);
        drop(img);
        thread::spawn(move || {
            if let Some(text) = crate::ocr::recognize_payload(&helper, data) {
                if let Some(shared) = SHARED.get() {
                    let mut st = shared.state.lock().unwrap();
                    let found = st.set_ocr(id, text);
                    let message = found.map(|f| st.sensitive_message(f));
                    if let Some(m) = &message {
                        st.notice = Some(m.clone());
                    }
                    // Soltar el estado antes de tocar egui: al cerrar, egui toma su candado y
                    // luego el del estado; al revés, los dos hilos se esperarían para siempre.
                    drop(st);
                    if let Some(m) = message {
                        balloon(&m);
                        reschedule_expiry();
                    }
                    shared.ctx.request_repaint();
                }
            }
        });
    }
    true
}

unsafe fn read_clipboard(owner: HWND) -> Option<Captured> {
    unsafe {
        // Primero abrir y después mirar las marcas: así lo que se lee es lo mismo que se revisó
        // (si otra app copia justo en medio, como un gestor de contraseñas, no se cuela).
        if !open_clipboard(owner) {
            return None;
        }
        let _guard = ClipboardGuard;
        // Convención de Windows: los gestores de contraseñas marcan así lo que no se debe guardar.
        for name in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "Clipboard Viewer Ignore",
        ] {
            if IsClipboardFormatAvailable(format_id(name)) != 0 {
                return None;
            }
        }
        let can_include = format_id("CanIncludeInClipboardHistory");
        if IsClipboardFormatAvailable(can_include) != 0 {
            let h = GetClipboardData(can_include);
            let p = GlobalLock(h) as *const u32;
            if !p.is_null() {
                let allowed = GlobalSize(h) < 4 || *p != 0;
                GlobalUnlock(h);
                if !allowed {
                    return None;
                }
            }
        }
        // Si hay texto se guarda el texto (Excel, por ejemplo, también pone una imagen).
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
            if IsClipboardFormatAvailable(CF_DIB) == 0 {
                return None;
            }
            let h = GetClipboardData(CF_DIB);
            let p = GlobalLock(h) as *const u8;
            if p.is_null() {
                return None;
            }
            // Se valida la cabecera antes de copiar: una imagen gigante o mentirosa no se
            // copia entera a memoria para después descartarla.
            let size = GlobalSize(h);
            let header = std::slice::from_raw_parts(p, size.min(image::DIB_HEADER_PEEK));
            let dib = image::dib_len(header)
                .filter(|&len| len <= size)
                .map(|len| std::slice::from_raw_parts(p, len).to_vec());
            GlobalUnlock(h);
            return dib.map(Captured::Dib);
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let p = GlobalLock(h) as *const u16;
        if p.is_null() {
            return None;
        }
        // El largo real es hasta el primer nulo (el bloque puede ser más grande que el texto).
        let max = GlobalSize(h) / 2;
        let len = (0..max).find(|&i| *p.add(i) == 0).unwrap_or(max);
        let text = (len <= MAX_TEXT_BYTES)
            .then(|| String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
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

/// Para pega-cli: una ventana de solo mensajes como dueña del portapapeles (sin ella,
/// Windows no deja escribir). No escucha nada ni registra atajos.
pub fn init_cli_clipboard() {
    unsafe {
        let class = wide("PegaCli");
        let hinstance = GetModuleHandleW(ptr::null());
        let mut wc: WNDCLASSW = mem::zeroed();
        wc.lpfnWndProc = Some(DefWindowProcW);
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
    }
}

/// Texto que hay ahora en el portapapeles (para la variable {portapapeles}).
pub fn current_text() -> Option<String> {
    match unsafe { read_clipboard(LISTENER_HWND.load(Relaxed) as HWND) } {
        Some(Captured::Text(text)) => Some(text),
        _ => None,
    }
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
        // Con Ctrl+1…9 la tecla sigue apretada: el destino recibiría Ctrl+Shift+V o similar.
        wait_modifiers_released();
        thread::sleep(Duration::from_millis(60));
        SetForegroundWindow(prev);
        thread::sleep(Duration::from_millis(40));
        send_ctrl_v();
    });
}

/// Espera (hasta 2 s) a que se suelten Ctrl, Alt, Shift y Win: si no, el Ctrl+V que
/// mandamos llegaría combinado con las teclas del atajo que el usuario aún sostiene.
fn wait_modifiers_released() {
    for _ in 0..100 {
        let held = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
            .iter()
            .any(|&k| unsafe { GetAsyncKeyState(k as i32) } < 0);
        if !held {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Atajo global: lo copiado pasa a texto plano (sin formato, sin imágenes) y se pega
/// en la ventana que esté al frente.
fn paste_plain() {
    let Some(text) = current_text() else { return };
    thread::spawn(move || {
        wait_modifiers_released();
        if write_text(&text, false) {
            thread::sleep(Duration::from_millis(30));
            unsafe { send_ctrl_v() };
        }
    });
}

/// Pega varios textos seguidos en la ventana anterior, con Tab entre uno y otro
/// (sirve para llenar formularios campo por campo).
pub fn paste_sequence(ctx: &egui::Context, items: Vec<(String, bool)>) {
    hide(ctx);
    let prev = PREV_FG.load(Relaxed);
    thread::spawn(move || unsafe {
        let prev = prev as HWND;
        if prev.is_null() || IsWindow(prev) == 0 {
            return;
        }
        wait_modifiers_released();
        thread::sleep(Duration::from_millis(60));
        SetForegroundWindow(prev);
        thread::sleep(Duration::from_millis(60));
        let last = items.len().saturating_sub(1);
        for (i, (text, private)) in items.iter().enumerate() {
            if !write_text(text, *private) {
                break;
            }
            thread::sleep(Duration::from_millis(30));
            send_ctrl_v();
            // Tiempo para que la app procese el pegado antes de cambiar el portapapeles.
            thread::sleep(Duration::from_millis(150));
            if i < last {
                let tab = [key(VK_TAB, false), key(VK_TAB, true)];
                SendInput(
                    tab.len() as u32,
                    tab.as_ptr(),
                    mem::size_of::<INPUT>() as i32,
                );
                thread::sleep(Duration::from_millis(60));
            }
        }
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
