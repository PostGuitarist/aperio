//! The models this plugin uses and how to feed them. Downloading and
//! loading them is shared with other AI plugins (see [`aperio_ai`]).

pub use aperio_ai::{default_dir, ModelFile};

/// A segmentation model: its file and how the network expects its input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub file: ModelFile,
    /// Square input size the network expects.
    pub input_size: u32,
    /// Per-channel mean and standard deviation the network was trained
    /// with, for sRGB values in 0..1.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Whether the output is a logit (needs a sigmoid) or already 0..1.
    pub logits: bool,
}

/// ImageNet statistics, which most segmentation models are trained with.
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// U²-Net sky segmentation by xiongzhu666, MIT.
pub const SKY: Model = Model {
    file: ModelFile {
        id: "skyseg",
        name: "Sky segmentation (U²-Net)",
        license: "MIT",
        source: "https://github.com/xiongzhu666/Sky-Segmentation-and-Post-processing",
        url: "https://huggingface.co/JianyuanWang/skyseg/resolve/3ba8c6df1d9ba9ff26f637c7ba9568ac11a9aa7f/skyseg.onnx",
        sha256: "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
        size_bytes: 175_997_079,
        optional: false,
        notice: None,
    },
    input_size: 320,
    mean: IMAGENET_MEAN,
    std: IMAGENET_STD,
    logits: false,
};

/// BiRefNet lite (salient object / subject segmentation), MIT.
pub const SUBJECT: Model = Model {
    file: ModelFile {
        id: "birefnet_lite",
        name: "Subject segmentation (BiRefNet lite)",
        license: "MIT",
        source: "https://github.com/ZhengPeng7/BiRefNet",
        url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx",
        sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
        size_bytes: 224_005_088,
        optional: false,
        notice: None,
    },
    input_size: 1024,
    mean: IMAGENET_MEAN,
    std: IMAGENET_STD,
    logits: true,
};

/// ISNet general use (DIS), a much faster subject model with softer edges.
/// Optional: the weights state no license and were trained on DIS5K, whose
/// terms are non-commercial (see LICENSES.md). This is rembg's export of the
/// authors' released weights.
pub const FAST_SUBJECT: Model = Model {
    file: ModelFile {
        id: "isnet_general_use",
        name: "Fast subject segmentation (ISNet)",
        license: "No license stated",
        source: "https://github.com/xuebinqin/DIS",
        url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx",
        sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
        size_bytes: 178_648_008,
        optional: true,
        notice: Some(
            "About 15 times faster than BiRefNet, with softer edges. Its authors \
             haven't stated a license for the weights, and they were trained on \
             DIS5K, whose terms allow only non-commercial research and education. \
             Download it only if that's acceptable for how you use your photos.",
        ),
    },
    input_size: 1024,
    mean: [0.5; 3],
    std: [1.0; 3],
    logits: false,
};

/// The models this build offers. ISNet is only offered with the
/// `fast-subject` feature, which the app and CLI turn on by default.
#[cfg(feature = "fast-subject")]
pub const ALL: [Model; 3] = [SKY, SUBJECT, FAST_SUBJECT];
#[cfg(not(feature = "fast-subject"))]
pub const ALL: [Model; 2] = [SKY, SUBJECT];

/// Whether this build offers [`FAST_SUBJECT`].
pub const FAST_SUBJECT_ENABLED: bool = cfg!(feature = "fast-subject");

/// Look a model up by id.
pub fn find(id: &str) -> Option<Model> {
    ALL.into_iter().find(|m| m.file.id == id)
}
