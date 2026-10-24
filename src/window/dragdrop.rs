//! OLE drag and drop.
//!
//! * [`DropTarget`] (`IDropTarget`) accepts files dragged from Explorer onto
//!   the notch and shows the shell drag image through `IDropTargetHelper`.
//! * [`drag_file_out`] starts a drag of a Shelf item with our own
//!   `IDropSource` and the shell's data object for the file, so it can be
//!   dropped into any application.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINT, POINTL, S_OK,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropSource,
    IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    BHID_DataObject, CLSID_DragDropHelper, DragQueryFileW, HDROP, IDropTargetHelper, IShellItem,
    SHCreateItemFromParsingName, SHDoDragDrop,
};
use windows::core::{BOOL, HRESULT, HSTRING, Ref, implement};

use crate::app::UserEvent;

/// Set while one of our own items is being dragged out, so dropping it back
/// onto the notch is ignored.
static DRAGGING_OUT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub enum DragEvent {
    Enter,
    Leave,
    Drop(Vec<PathBuf>),
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
    helper: Option<IDropTargetHelper>,
    accepting: std::cell::Cell<bool>,
}

fn has_files(data: &IDataObject) -> bool {
    let format = hdrop_format();
    unsafe { data.QueryGetData(&format) == S_OK }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn dropped_files(data: &IDataObject) -> Vec<PathBuf> {
    let format = hdrop_format();
    let mut files = Vec::new();
    unsafe {
        let Ok(mut medium) = data.GetData(&format) else { return files };
        let hdrop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        for index in 0..count {
            let len = DragQueryFileW(hdrop, index, None) as usize;
            let mut buffer = vec![0u16; len + 1];
            let copied = DragQueryFileW(hdrop, index, Some(&mut buffer)) as usize;
            files.push(PathBuf::from(OsString::from_wide(&buffer[..copied.min(len)])));
        }
        ReleaseStgMedium(&mut medium);
    }
    files
}

fn set_effect(effect: *mut DROPEFFECT, value: DROPEFFECT) {
    if !effect.is_null() {
        unsafe { *effect = value };
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accept = !DRAGGING_OUT.load(Ordering::Relaxed) && data.as_ref().is_some_and(has_files);
        self.accepting.set(accept);
        let value = if accept { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        set_effect(effect, value);
        if let (Some(helper), Some(data)) = (&self.helper, data.as_ref()) {
            let pt = POINT { x: point.x, y: point.y };
            unsafe {
                let _ = helper.DragEnter(self.hwnd, data, &pt, value);
            }
        }
        if accept {
            super::win32::post(UserEvent::Drag(DragEvent::Enter));
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, point: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let value = if self.accepting.get() { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        set_effect(effect, value);
        if let Some(helper) = &self.helper {
            let pt = POINT { x: point.x, y: point.y };
            unsafe {
                let _ = helper.DragOver(&pt, value);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragLeave();
            }
        }
        if self.accepting.replace(false) {
            super::win32::post(UserEvent::Drag(DragEvent::Leave));
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let accept = self.accepting.replace(false);
        let value = if accept { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        set_effect(effect, value);
        if let (Some(helper), Some(data)) = (&self.helper, data.as_ref()) {
            let pt = POINT { x: point.x, y: point.y };
            unsafe {
                let _ = helper.Drop(data, &pt, value);
            }
        }
        if accept {
            let files = data.as_ref().map(dropped_files).unwrap_or_default();
            super::win32::post(UserEvent::Drag(DragEvent::Drop(files)));
        }
        Ok(())
    }
}

/// Keeps the drop target registered for the lifetime of the window.
pub struct DropRegistration {
    hwnd: HWND,
}

impl DropRegistration {
    /// Requires OLE to be initialized on the calling (UI) thread.
    pub fn register(hwnd: HWND) -> windows::core::Result<Self> {
        let helper: Option<IDropTargetHelper> =
            unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
        let target: IDropTarget = DropTarget { hwnd, helper, accepting: std::cell::Cell::new(false) }.into();
        unsafe { RegisterDragDrop(hwnd, &target)? };
        Ok(Self { hwnd })
    }
}

impl Drop for DropRegistration {
    fn drop(&mut self) {
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
        }
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape_pressed: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape_pressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (keys & MK_LBUTTON).0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drags `path` out of the notch (modal until the drop finishes). Returns
/// the effect chosen by the drop target.
pub fn drag_file_out(hwnd: HWND, path: &Path) -> windows::core::Result<DROPEFFECT> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
        let data: IDataObject = item.BindToHandler(None, &BHID_DataObject)?;
        let source: IDropSource = DropSource.into();
        DRAGGING_OUT.store(true, Ordering::Relaxed);
        let result = SHDoDragDrop(Some(hwnd), &data, &source, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK);
        DRAGGING_OUT.store(false, Ordering::Relaxed);
        result
    }
}
