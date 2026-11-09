// The taskbar's right-click menu: recent project folders and "New Window".
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Storage::EnhancedStorage::PKEY_Title;
use windows::Win32::System::Com::StructuredStorage::{InitPropVariantFromStringVector, PropVariantClear};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, SetCurrentProcessExplicitAppUserModelID, ShellLink};

/// The taskbar groups by this id, and pinned shortcuts and notifications both depend on it
pub fn set_app_id(id: &str) {
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(id)) };
}

fn link(exe: &str, args: &str, title: &str, description: &str) -> windows::core::Result<IShellLinkW> {
    unsafe {
        let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        l.SetPath(&HSTRING::from(exe))?;
        l.SetArguments(&HSTRING::from(args))?;
        l.SetDescription(&HSTRING::from(description))?;
        l.SetIconLocation(&HSTRING::from(exe), 0)?;
        let store: IPropertyStore = l.cast()?;
        let title_w = HSTRING::from(title);
        let items = [PCWSTR(title_w.as_ptr())];
        let mut value = InitPropVariantFromStringVector(Some(&items))?;
        // A title needs a plain string, which the vector form with one entry converts to
        store.SetValue(&PKEY_Title, &value)?;
        store.Commit()?;
        let _ = PropVariantClear(&mut value);
        Ok(l)
    }
}

fn collection(links: &[IShellLinkW]) -> windows::core::Result<IObjectArray> {
    unsafe {
        let c: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
        for l in links {
            c.AddObject(l)?;
        }
        c.cast()
    }
}

fn build(recent: &[String], language: Option<&str>) -> windows::core::Result<()> {
    let zh = language == Some("zh-TW") || (language.is_none() && std::env::var("LANG").map(|l| l.starts_with("zh")).unwrap_or(false));
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        let mut slots = 0u32;
        let removed: IObjectArray = list.BeginList(&mut slots)?;
        // Windows refuses a list that re-adds what the user removed
        let mut gone: Vec<String> = Vec::new();
        for i in 0..removed.GetCount().unwrap_or(0) {
            if let Ok(l) = removed.GetAt::<IShellLinkW>(i) {
                let mut buf = [0u16; 1024];
                if l.GetArguments(&mut buf).is_ok() {
                    let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
                    gone.push(String::from_utf16_lossy(&buf[..n]).trim_matches('"').to_lowercase());
                }
            }
        }
        let links: Vec<IShellLinkW> = recent
            .iter()
            .filter(|d| !gone.contains(&d.to_lowercase()))
            .take(10)
            .filter_map(|dir| {
                let title = wb_core::util::basename(dir);
                link(&exe, &format!("\"{dir}\""), if title.is_empty() { dir } else { &title }, dir).ok()
            })
            .collect();
        if !links.is_empty() {
            // Denied when the user turned off recent items in Windows privacy settings; the task below still works
            let _ = list.AppendCategory(&HSTRING::from(if zh { "最近開啟的資料夾" } else { "Recent Folders" }), &collection(&links)?);
        }
        let new_window = link(&exe, "--new-window", if zh { "開啟新視窗" } else { "New Window" }, if zh { "開啟全新的專案視窗" } else { "Open a new project window" })?;
        list.AddUserTasks(&collection(&[new_window])?)?;
        list.CommitList()
    }
}

pub fn update(recent: &[String], language: Option<&str>) {
    let (recent, language) = (recent.to_vec(), language.map(str::to_string));
    // COM on its own thread: this is called from anywhere
    std::thread::spawn(move || {
        if let Err(e) = build(&recent, language.as_deref()) {
            eprintln!("[jump list] {e}");
        }
    });
}
