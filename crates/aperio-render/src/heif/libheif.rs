//! HEIF decoding with libheif, loaded at run time.
//!
//! Aperio doesn't link libheif or ship it: it uses the system's copy if
//! there is one (for example the `libheif1` and `libheif-plugin-libde265`
//! packages on Debian and Ubuntu), so the HEVC decoder comes from the
//! distribution. Only the handful of libheif's C functions used here are
//! declared; they've been stable since libheif 1.x.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use libloading::Library;

use super::color::Conversion;
use crate::{Error, LinearImage, Result};

const COLORSPACE_RGB: c_int = 1;
const CHROMA_RGBA: c_int = 11;
const CHROMA_RRGGBBAA_LE: c_int = 15;
const CHANNEL_INTERLEAVED: c_int = 10;
const PRIMARIES_UNSPECIFIED: c_int = 2;

#[repr(C)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

#[repr(C)]
struct Nclx {
    version: u8,
    color_primaries: c_int,
    transfer_characteristics: c_int,
    matrix_coefficients: c_int,
    full_range_flag: u8,
    red: [f32; 2],
    green: [f32; 2],
    blue: [f32; 2],
    white: [f32; 2],
}

type Ctx = *mut c_void;
type Handle = *mut c_void;
type Image = *mut c_void;

/// libheif's functions, with the library kept loaded for as long as the
/// process runs.
struct Api {
    _lib: Library,
    context_alloc: unsafe extern "C" fn() -> Ctx,
    context_free: unsafe extern "C" fn(Ctx),
    read_from_file: unsafe extern "C" fn(Ctx, *const c_char, *const c_void) -> HeifError,
    primary_handle: unsafe extern "C" fn(Ctx, *mut Handle) -> HeifError,
    handle_release: unsafe extern "C" fn(Handle),
    luma_bits: unsafe extern "C" fn(Handle) -> c_int,
    raw_profile_size: unsafe extern "C" fn(Handle) -> usize,
    raw_profile: unsafe extern "C" fn(Handle, *mut c_void) -> HeifError,
    nclx_profile: unsafe extern "C" fn(Handle, *mut *mut Nclx) -> HeifError,
    nclx_free: unsafe extern "C" fn(*mut Nclx),
    decode_image:
        unsafe extern "C" fn(Handle, *mut Image, c_int, c_int, *const c_void) -> HeifError,
    image_release: unsafe extern "C" fn(Image),
    image_width: unsafe extern "C" fn(Image, c_int) -> c_int,
    image_height: unsafe extern "C" fn(Image, c_int) -> c_int,
    image_bits: unsafe extern "C" fn(Image, c_int) -> c_int,
    plane: unsafe extern "C" fn(Image, c_int, *mut c_int) -> *const u8,
}

const NOT_INSTALLED: &str = "opening HEIC photos needs libheif and its HEVC decoder plugin \
     (on Debian or Ubuntu: sudo apt install libheif1 libheif-plugin-libde265)";

fn api() -> Result<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe { load().ok() })
        .as_ref()
        .ok_or_else(|| Error::Heif(NOT_INSTALLED.into()))
}

unsafe fn load() -> std::result::Result<Api, libloading::Error> {
    let lib = Library::new("libheif.so.1").or_else(|_| Library::new("libheif.so"))?;
    // heif_init loads the decoder plugins (libheif 1.14 and later). It's
    // reference counted; Aperio keeps it initialized until it exits.
    if let Ok(init) = lib.get::<unsafe extern "C" fn(*const c_void) -> HeifError>(b"heif_init\0") {
        init(null());
    }
    macro_rules! sym {
        ($name:literal) => {
            *lib.get(concat!($name, "\0").as_bytes())?
        };
    }
    Ok(Api {
        context_alloc: sym!("heif_context_alloc"),
        context_free: sym!("heif_context_free"),
        read_from_file: sym!("heif_context_read_from_file"),
        primary_handle: sym!("heif_context_get_primary_image_handle"),
        handle_release: sym!("heif_image_handle_release"),
        luma_bits: sym!("heif_image_handle_get_luma_bits_per_pixel"),
        raw_profile_size: sym!("heif_image_handle_get_raw_color_profile_size"),
        raw_profile: sym!("heif_image_handle_get_raw_color_profile"),
        nclx_profile: sym!("heif_image_handle_get_nclx_color_profile"),
        nclx_free: sym!("heif_nclx_color_profile_free"),
        decode_image: sym!("heif_decode_image"),
        image_release: sym!("heif_image_release"),
        image_width: sym!("heif_image_get_width"),
        image_height: sym!("heif_image_get_height"),
        image_bits: sym!("heif_image_get_bits_per_pixel_range"),
        plane: sym!("heif_image_get_plane_readonly"),
        _lib: lib,
    })
}

fn check(err: HeifError) -> Result<()> {
    if err.code == 0 {
        return Ok(());
    }
    let message = if err.message.is_null() {
        format!("libheif error {}", err.code)
    } else {
        unsafe { CStr::from_ptr(err.message) }
            .to_string_lossy()
            .into_owned()
    };
    Err(Error::Heif(message))
}

/// Calls `free` on `ptr` when dropped.
struct Guard<T>(*mut T, unsafe extern "C" fn(*mut T));

impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (self.1)(self.0) }
        }
    }
}

pub fn decode(path: &Path) -> Result<LinearImage> {
    let api = api()?;
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Heif("path contains a NUL byte".into()))?;
    unsafe {
        let ctx = Guard((api.context_alloc)(), api.context_free);
        if ctx.0.is_null() {
            return Err(Error::Heif("libheif could not allocate a context".into()));
        }
        check((api.read_from_file)(ctx.0, c_path.as_ptr(), null()))?;
        let mut handle = null_mut();
        check((api.primary_handle)(ctx.0, &mut handle))?;
        let handle = Guard(handle, api.handle_release);

        let conversion = conversion(api, handle.0);
        let deep = (api.luma_bits)(handle.0) > 8;
        let chroma = if deep {
            CHROMA_RRGGBBAA_LE
        } else {
            CHROMA_RGBA
        };
        // libheif applies the photo's rotation and mirroring while decoding.
        let mut image = null_mut();
        check((api.decode_image)(
            handle.0,
            &mut image,
            COLORSPACE_RGB,
            chroma,
            null(),
        ))?;
        let image = Guard(image, api.image_release);

        let width = (api.image_width)(image.0, CHANNEL_INTERLEAVED);
        let height = (api.image_height)(image.0, CHANNEL_INTERLEAVED);
        let mut stride: c_int = 0;
        let data = (api.plane)(image.0, CHANNEL_INTERLEAVED, &mut stride);
        if data.is_null() || width <= 0 || height <= 0 {
            return Err(Error::Heif("libheif returned no pixels".into()));
        }
        let (width, height, stride) = (width as usize, height as usize, stride as usize);
        let bytes = std::slice::from_raw_parts(
            data,
            stride * (height - 1) + width * if deep { 8 } else { 4 },
        );

        let mut pixels = Vec::with_capacity(width * height);
        if deep {
            let max =
                ((1u32 << (api.image_bits)(image.0, CHANNEL_INTERLEAVED).clamp(9, 16)) - 1) as f32;
            for y in 0..height {
                let row = &bytes[y * stride..y * stride + width * 8];
                pixels.extend(row.as_chunks::<8>().0.iter().map(|p| {
                    let v = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]) as f32 / max;
                    conversion.apply([v(0), v(2), v(4), v(6)])
                }));
            }
        } else {
            for y in 0..height {
                let row = &bytes[y * stride..y * stride + width * 4];
                pixels.extend(
                    row.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| conversion.apply(p.map(|v| v as f32 / 255.0))),
                );
            }
        }
        Ok(LinearImage::new(width as u32, height as u32, pixels))
    }
}

/// The photo's color space: its ICC profile if it has one, otherwise its
/// `nclx` primaries, otherwise sRGB.
unsafe fn conversion(api: &Api, handle: Handle) -> Conversion {
    let size = (api.raw_profile_size)(handle);
    if size > 0 {
        let mut icc = vec![0u8; size];
        if check((api.raw_profile)(handle, icc.as_mut_ptr().cast())).is_ok() {
            if let Some(c) = Conversion::from_icc(&icc) {
                return c;
            }
        }
    }
    let mut nclx = null_mut();
    if check((api.nclx_profile)(handle, &mut nclx)).is_ok() && !nclx.is_null() {
        let nclx = Guard(nclx, api.nclx_free);
        let p = &*nclx.0;
        if p.color_primaries != PRIMARIES_UNSPECIFIED {
            return Conversion::from_primaries(p.red, p.green, p.blue, p.white);
        }
    }
    Conversion::SRGB
}
