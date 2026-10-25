//! HEIF decoding with ImageIO, which macOS has built in.

use std::ffi::c_void;
use std::path::Path;

use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize, CFURL,
};
use objc2_core_graphics::{
    kCGColorSpaceExtendedLinearSRGB, CGBitmapContextCreate, CGColorSpace, CGContext, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGImageComponentInfo,
};
use objc2_image_io::{
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform,
    kCGImageSourceShouldCacheImmediately, kCGImageSourceThumbnailMaxPixelSize, CGImageSource,
};

use crate::{Error, LinearImage, Result};

fn fail(what: &str) -> Error {
    Error::Heif(format!("ImageIO could not {what}"))
}

pub fn decode(path: &Path) -> Result<LinearImage> {
    let url = CFURL::from_file_path(path).ok_or_else(|| fail("read the file's path"))?;
    unsafe {
        let source = CGImageSource::with_url(&url, None).ok_or_else(|| fail("open the file"))?;
        let image = upright_image(&source)?;
        draw_linear(&image)
    }
}

/// The photo's primary image at full size, upright. ImageIO only applies the
/// photo's rotation and mirroring when making a "thumbnail", so this asks
/// for one as large as the photo itself.
unsafe fn upright_image(source: &CGImageSource) -> Result<CFRetained<CGImage>> {
    let raw = source
        .image_at_index(0, None)
        .ok_or_else(|| fail("decode the photo"))?;
    let edge = CGImage::width(Some(&raw)).max(CGImage::height(Some(&raw)));
    let yes = CFBoolean::new(true);
    let edge = CFNumber::new_isize(edge as isize);
    let keys: [&CFString; 4] = [
        kCGImageSourceCreateThumbnailFromImageAlways,
        kCGImageSourceCreateThumbnailWithTransform,
        kCGImageSourceShouldCacheImmediately,
        kCGImageSourceThumbnailMaxPixelSize,
    ];
    let values: [&CFType; 4] = [yes.as_ref(), yes.as_ref(), yes.as_ref(), edge.as_ref()];
    let options = CFDictionary::from_slices(&keys, &values);
    source
        .thumbnail_at_index(0, Some(options.as_opaque()))
        .ok_or_else(|| fail("decode the photo"))
}

/// Draw `image` into a float bitmap in extended linear sRGB, which has
/// Core Graphics convert from the photo's own color space (Display P3 for
/// iPhones) and linearize it.
unsafe fn draw_linear(image: &CGImage) -> Result<LinearImage> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let space = CGColorSpace::with_name(Some(kCGColorSpaceExtendedLinearSRGB))
        .ok_or_else(|| fail("create a color space"))?;
    let mut pixels = vec![[0f32; 4]; width * height];
    let info = CGImageAlphaInfo::PremultipliedLast.0
        | CGImageComponentInfo::Float.0
        | CGImageByteOrderInfo::Order32Little.0;
    let context: CFRetained<CGContext> = CGBitmapContextCreate(
        pixels.as_mut_ptr().cast::<c_void>(),
        width,
        height,
        32,
        width * 16,
        Some(&space),
        info,
    )
    .ok_or_else(|| fail("create a bitmap"))?;
    let rect = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as f64, height as f64),
    );
    CGContext::draw_image(Some(&context), rect, Some(image));
    drop(context);
    for p in &mut pixels {
        if p[3] > 0.0 && p[3] < 1.0 {
            for c in 0..3 {
                p[c] /= p[3];
            }
        }
    }
    Ok(LinearImage::new(width as u32, height as u32, pixels))
}
