//! HEIF decoding with the Windows Imaging Component (WIC). Windows decodes
//! HEIC once Microsoft's HEIF Image Extensions and HEVC Video Extensions are
//! installed from the Microsoft Store.

use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Foundation::{GENERIC_READ, WINCODEC_ERR_COMPONENTNOTFOUND};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat64bppRGBA, IWICBitmapFrameDecode, IWICColorContext,
    IWICImagingFactory, IWICPalette, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom,
    WICColorContextProfile, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use super::color::Conversion;
use crate::{Error, LinearImage, Result};

const NOT_INSTALLED: &str = "Windows needs Microsoft's HEIF Image Extensions and HEVC Video \
     Extensions (from the Microsoft Store) to open HEIC photos";

fn wic(e: windows::core::Error) -> Error {
    if e.code() == WINCODEC_ERR_COMPONENTNOTFOUND {
        Error::Heif(NOT_INSTALLED.into())
    } else {
        Error::Heif(format!("Windows could not decode the photo: {e}"))
    }
}

pub fn decode(path: &Path) -> Result<LinearImage> {
    unsafe {
        // COM may already be set up on this thread (in either mode), which
        // is fine; only undo what this call did.
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = decode_with_com(path);
        if com {
            CoUninitialize();
        }
        result
    }
}

unsafe fn decode_with_com(path: &Path) -> Result<LinearImage> {
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(wic)?;
    let decoder = factory
        .CreateDecoderFromFilename(
            &HSTRING::from(path),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )
        .map_err(wic)?;
    let frame = decoder.GetFrame(0).map_err(wic)?;
    let conversion = conversion(&factory, &frame);

    // 16 bits per channel, still sRGB-encoded, so 10-bit photos keep their
    // precision.
    let converter = factory.CreateFormatConverter().map_err(wic)?;
    converter
        .Initialize(
            &frame,
            &GUID_WICPixelFormat64bppRGBA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(wic)?;
    let (mut width, mut height) = (0u32, 0u32);
    converter.GetSize(&mut width, &mut height).map_err(wic)?;
    let stride = width as usize * 8;
    let mut bytes = vec![0u8; stride * height as usize];
    converter
        .CopyPixels(std::ptr::null(), stride as u32, &mut bytes)
        .map_err(wic)?;
    let pixels = bytes
        .chunks_exact(8)
        .map(|p| {
            let v = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]) as f32 / 65535.0;
            conversion.apply([v(0), v(2), v(4), v(6)])
        })
        .collect();
    let image = LinearImage::new(width, height, pixels);
    Ok(orient(path, image))
}

/// The frame's color space, from its ICC profile when it has one.
unsafe fn conversion(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode) -> Conversion {
    let icc = || -> windows::core::Result<Option<Vec<u8>>> {
        let mut count = 0u32;
        frame.GetColorContexts(&mut [], &mut count)?;
        let mut contexts: Vec<Option<IWICColorContext>> = (0..count)
            .map(|_| factory.CreateColorContext().map(Some))
            .collect::<windows::core::Result<_>>()?;
        frame.GetColorContexts(&mut contexts, &mut count)?;
        for context in contexts.iter().flatten() {
            if context.GetType()? != WICColorContextProfile {
                continue;
            }
            let mut size = 0u32;
            context.GetProfileBytes(&mut [], &mut size)?;
            let mut bytes = vec![0u8; size as usize];
            context.GetProfileBytes(&mut bytes, &mut size)?;
            return Ok(Some(bytes));
        }
        Ok(None)
    };
    match icc() {
        Ok(Some(bytes)) => Conversion::from_icc(&bytes).unwrap_or(Conversion::SRGB),
        _ => Conversion::SRGB,
    }
}

/// Turn the photo upright. WIC's HEIF decoder returns the stored pixels and
/// leaves the rotation to the caller; the EXIF orientation matches the
/// file's own rotation for cameras and phones. In case a decoder has already
/// rotated the photo, a turn is skipped when the decoded size is already
/// the EXIF size turned on its side.
fn orient(path: &Path, image: LinearImage) -> LinearImage {
    use exif::{In, Tag};
    let Ok(file) = std::fs::File::open(path) else {
        return image;
    };
    let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file))
    else {
        return image;
    };
    let get = |tag| {
        exif.get_field(tag, In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
    };
    let Some(orientation) = get(Tag::Orientation) else {
        return image;
    };
    let (transpose, hflip, vflip) =
        rawler::decoders::Orientation::from_u16(orientation as u16).to_flips();
    if transpose {
        if let (Some(w), Some(h)) = (get(Tag::PixelXDimension), get(Tag::PixelYDimension)) {
            if (image.width, image.height) == (h, w) && w != h {
                return image;
            }
        }
    }
    image.orient(transpose, hflip, vflip)
}
