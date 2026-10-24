//! File metadata and Explorer thumbnails for items on the Shelf.
//!
//! Thumbnails come from `IShellItemImageFactory`, the same source Explorer
//! uses (image previews, document thumbnails, app icons), on a tokio
//! blocking worker initialized as a COM single-threaded apartment.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use tokio::runtime::Handle;
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW,
    HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK};
use windows::core::HSTRING;

use super::{EventBus, ServiceEvent};

const THUMBNAIL_SIZE: i32 = 96;

#[derive(Debug)]
pub struct ShelfFileInfo {
    pub id: u64,
    pub exists: bool,
    pub name: String,
    pub size: Option<u64>,
    pub thumbnail: Option<Arc<ImageData>>,
}

#[derive(Clone)]
pub struct ShelfHandle(Sender<(u64, PathBuf)>);

impl ShelfHandle {
    /// Requests metadata and a thumbnail for a shelf item.
    pub fn inspect(&self, id: u64, path: PathBuf) {
        let _ = self.0.send((id, path));
    }
}

pub fn spawn(rt: &Handle, bus: EventBus) -> ShelfHandle {
    let (tx, rx) = mpsc::channel::<(u64, PathBuf)>();
    rt.spawn_blocking(move || {
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        while let Ok((id, path)) = rx.recv() {
            bus.send(ServiceEvent::ShelfFile(inspect(id, &path)));
        }
        if com.is_ok() {
            unsafe { CoUninitialize() };
        }
    });
    ShelfHandle(tx)
}

fn inspect(id: u64, path: &Path) -> ShelfFileInfo {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let metadata = std::fs::metadata(path).ok();
    let exists = metadata.is_some();
    let size = metadata.as_ref().filter(|m| m.is_file()).map(|m| m.len());
    let thumbnail = if exists {
        match shell_thumbnail(path) {
            Ok(image) => Some(Arc::new(image)),
            Err(err) => {
                log::debug!("no thumbnail for {}: {err}", path.display());
                None
            }
        }
    } else {
        None
    };
    ShelfFileInfo { id, exists, name, size, thumbnail }
}

fn shell_thumbnail(path: &Path) -> windows::core::Result<ImageData> {
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
        let bitmap = factory.GetImage(SIZE { cx: THUMBNAIL_SIZE, cy: THUMBNAIL_SIZE }, SIIGBF_BIGGERSIZEOK)?;
        let result = bitmap_to_image(bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        result
    }
}

/// Copies a 32-bit DIB into straight-alpha RGBA.
unsafe fn bitmap_to_image(bitmap: HBITMAP) -> windows::core::Result<ImageData> {
    let mut info = BITMAP::default();
    let read = unsafe {
        GetObjectW(HGDIOBJ(bitmap.0), size_of::<BITMAP>() as i32, Some(&mut info as *mut BITMAP as *mut std::ffi::c_void))
    };
    if read == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
        return Err(windows::core::Error::from_thread());
    }
    let (width, height) = (info.bmWidth, info.bmHeight.abs());
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let rows = unsafe {
        let dc = GetDC(None);
        let rows = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut std::ffi::c_void),
            &mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        rows
    };
    if rows == 0 {
        return Err(windows::core::Error::from_thread());
    }
    // Thumbnails without an alpha channel report alpha = 0 everywhere.
    let opaque = pixels.as_chunks::<4>().0.iter().all(|px| px[3] == 0);
    for px in pixels.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        if opaque {
            px[3] = 255;
        } else if px[3] > 0 && px[3] < 255 {
            // Shell bitmaps are premultiplied.
            let a = px[3] as u32;
            for channel in &mut px[..3] {
                *channel = ((*channel as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(ImageData {
        data: Blob::from(pixels),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: width as u32,
        height: height as u32,
    })
}

/// Human readable size: "2.4 MB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else if value < 10.0 { format!("{value:.1} {}", UNITS[unit]) } else { format!("{value:.0} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2_516_582), "2.4 MB");
        assert_eq!(format_size(52_428_800), "50 MB");
    }
}
