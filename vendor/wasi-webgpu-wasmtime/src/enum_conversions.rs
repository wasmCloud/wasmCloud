use crate::wasi::webgpu::webgpu;

impl From<webgpu::GpuPowerPreference> for wgpu_types::PowerPreference {
    fn from(value: webgpu::GpuPowerPreference) -> Self {
        match value {
            webgpu::GpuPowerPreference::LowPower => wgpu_types::PowerPreference::LowPower,
            webgpu::GpuPowerPreference::HighPerformance => {
                wgpu_types::PowerPreference::HighPerformance
            }
        }
    }
}

impl From<wgpu_types::TextureFormat> for webgpu::GpuTextureFormat {
    fn from(value: wgpu_types::TextureFormat) -> Self {
        match value {
            wgpu_types::TextureFormat::Bgra8UnormSrgb => webgpu::GpuTextureFormat::Bgra8unormSrgb,
            wgpu_types::TextureFormat::R8Unorm => webgpu::GpuTextureFormat::R8unorm,
            wgpu_types::TextureFormat::R8Snorm => webgpu::GpuTextureFormat::R8snorm,
            wgpu_types::TextureFormat::R8Uint => webgpu::GpuTextureFormat::R8uint,
            wgpu_types::TextureFormat::R8Sint => webgpu::GpuTextureFormat::R8sint,
            wgpu_types::TextureFormat::R16Uint => webgpu::GpuTextureFormat::R16uint,
            wgpu_types::TextureFormat::R16Sint => webgpu::GpuTextureFormat::R16sint,
            wgpu_types::TextureFormat::R16Unorm => webgpu::GpuTextureFormat::R16unorm,
            wgpu_types::TextureFormat::R16Snorm => webgpu::GpuTextureFormat::R16snorm,
            wgpu_types::TextureFormat::R16Float => webgpu::GpuTextureFormat::R16float,
            wgpu_types::TextureFormat::Rg8Unorm => webgpu::GpuTextureFormat::Rg8unorm,
            wgpu_types::TextureFormat::Rg8Snorm => webgpu::GpuTextureFormat::Rg8snorm,
            wgpu_types::TextureFormat::Rg8Uint => webgpu::GpuTextureFormat::Rg8uint,
            wgpu_types::TextureFormat::Rg8Sint => webgpu::GpuTextureFormat::Rg8sint,
            wgpu_types::TextureFormat::R32Uint => webgpu::GpuTextureFormat::R32uint,
            wgpu_types::TextureFormat::R32Sint => webgpu::GpuTextureFormat::R32sint,
            wgpu_types::TextureFormat::R32Float => webgpu::GpuTextureFormat::R32float,
            wgpu_types::TextureFormat::R64Uint => todo!(),
            wgpu_types::TextureFormat::Rg16Uint => webgpu::GpuTextureFormat::Rg16uint,
            wgpu_types::TextureFormat::Rg16Sint => webgpu::GpuTextureFormat::Rg16sint,
            wgpu_types::TextureFormat::Rg16Unorm => webgpu::GpuTextureFormat::Rg16unorm,
            wgpu_types::TextureFormat::Rg16Snorm => webgpu::GpuTextureFormat::Rg16snorm,
            wgpu_types::TextureFormat::Rg16Float => webgpu::GpuTextureFormat::Rg16float,
            wgpu_types::TextureFormat::Rgba8Unorm => webgpu::GpuTextureFormat::Rgba8unorm,
            wgpu_types::TextureFormat::Rgba8UnormSrgb => webgpu::GpuTextureFormat::Rgba8unormSrgb,
            wgpu_types::TextureFormat::Rgba8Snorm => webgpu::GpuTextureFormat::Rgba8snorm,
            wgpu_types::TextureFormat::Rgba8Uint => webgpu::GpuTextureFormat::Rgba8uint,
            wgpu_types::TextureFormat::Rgba8Sint => webgpu::GpuTextureFormat::Rgba8sint,
            wgpu_types::TextureFormat::Bgra8Unorm => webgpu::GpuTextureFormat::Bgra8unorm,
            wgpu_types::TextureFormat::Rgb9e5Ufloat => webgpu::GpuTextureFormat::Rgb9e5ufloat,
            wgpu_types::TextureFormat::Rgb10a2Uint => webgpu::GpuTextureFormat::Rgb10a2uint,
            wgpu_types::TextureFormat::Rgb10a2Unorm => webgpu::GpuTextureFormat::Rgb10a2unorm,
            wgpu_types::TextureFormat::Rg11b10Ufloat => webgpu::GpuTextureFormat::Rg11b10ufloat,
            wgpu_types::TextureFormat::Rg32Uint => webgpu::GpuTextureFormat::Rg32uint,
            wgpu_types::TextureFormat::Rg32Sint => webgpu::GpuTextureFormat::Rg32sint,
            wgpu_types::TextureFormat::Rg32Float => webgpu::GpuTextureFormat::Rg32float,
            wgpu_types::TextureFormat::Rgba16Uint => webgpu::GpuTextureFormat::Rgba16uint,
            wgpu_types::TextureFormat::Rgba16Sint => webgpu::GpuTextureFormat::Rgba16sint,
            wgpu_types::TextureFormat::Rgba16Unorm => webgpu::GpuTextureFormat::Rgba16unorm,
            wgpu_types::TextureFormat::Rgba16Snorm => webgpu::GpuTextureFormat::Rgba16snorm,
            wgpu_types::TextureFormat::Rgba16Float => webgpu::GpuTextureFormat::Rgba16float,
            wgpu_types::TextureFormat::Rgba32Uint => webgpu::GpuTextureFormat::Rgba32uint,
            wgpu_types::TextureFormat::Rgba32Sint => webgpu::GpuTextureFormat::Rgba32sint,
            wgpu_types::TextureFormat::Rgba32Float => webgpu::GpuTextureFormat::Rgba32float,
            wgpu_types::TextureFormat::Stencil8 => webgpu::GpuTextureFormat::Stencil8,
            wgpu_types::TextureFormat::Depth16Unorm => webgpu::GpuTextureFormat::Depth16unorm,
            wgpu_types::TextureFormat::Depth24Plus => webgpu::GpuTextureFormat::Depth24plus,
            wgpu_types::TextureFormat::Depth24PlusStencil8 => {
                webgpu::GpuTextureFormat::Depth24plusStencil8
            }
            wgpu_types::TextureFormat::Depth32Float => webgpu::GpuTextureFormat::Depth32float,
            wgpu_types::TextureFormat::Depth32FloatStencil8 => {
                webgpu::GpuTextureFormat::Depth32floatStencil8
            }
            wgpu_types::TextureFormat::Bc1RgbaUnorm => webgpu::GpuTextureFormat::Bc1RgbaUnorm,
            wgpu_types::TextureFormat::Bc1RgbaUnormSrgb => {
                webgpu::GpuTextureFormat::Bc1RgbaUnormSrgb
            }
            wgpu_types::TextureFormat::Bc2RgbaUnorm => webgpu::GpuTextureFormat::Bc2RgbaUnorm,
            wgpu_types::TextureFormat::Bc2RgbaUnormSrgb => {
                webgpu::GpuTextureFormat::Bc2RgbaUnormSrgb
            }
            wgpu_types::TextureFormat::Bc3RgbaUnorm => webgpu::GpuTextureFormat::Bc3RgbaUnorm,
            wgpu_types::TextureFormat::Bc3RgbaUnormSrgb => {
                webgpu::GpuTextureFormat::Bc3RgbaUnormSrgb
            }
            wgpu_types::TextureFormat::Bc4RUnorm => webgpu::GpuTextureFormat::Bc4RUnorm,
            wgpu_types::TextureFormat::Bc4RSnorm => webgpu::GpuTextureFormat::Bc4RSnorm,
            wgpu_types::TextureFormat::Bc5RgUnorm => webgpu::GpuTextureFormat::Bc5RgUnorm,
            wgpu_types::TextureFormat::Bc5RgSnorm => webgpu::GpuTextureFormat::Bc5RgSnorm,
            wgpu_types::TextureFormat::Bc6hRgbUfloat => webgpu::GpuTextureFormat::Bc6hRgbUfloat,
            wgpu_types::TextureFormat::Bc6hRgbFloat => webgpu::GpuTextureFormat::Bc6hRgbFloat,
            wgpu_types::TextureFormat::Bc7RgbaUnorm => webgpu::GpuTextureFormat::Bc7RgbaUnorm,
            wgpu_types::TextureFormat::Bc7RgbaUnormSrgb => {
                webgpu::GpuTextureFormat::Bc7RgbaUnormSrgb
            }
            wgpu_types::TextureFormat::Etc2Rgb8Unorm => webgpu::GpuTextureFormat::Etc2Rgb8unorm,
            wgpu_types::TextureFormat::Etc2Rgb8UnormSrgb => {
                webgpu::GpuTextureFormat::Etc2Rgb8unormSrgb
            }
            wgpu_types::TextureFormat::Etc2Rgb8A1Unorm => webgpu::GpuTextureFormat::Etc2Rgb8a1unorm,
            wgpu_types::TextureFormat::Etc2Rgb8A1UnormSrgb => {
                webgpu::GpuTextureFormat::Etc2Rgb8a1unormSrgb
            }
            wgpu_types::TextureFormat::Etc2Rgba8Unorm => webgpu::GpuTextureFormat::Etc2Rgba8unorm,
            wgpu_types::TextureFormat::Etc2Rgba8UnormSrgb => {
                webgpu::GpuTextureFormat::Etc2Rgba8unormSrgb
            }
            wgpu_types::TextureFormat::EacR11Unorm => webgpu::GpuTextureFormat::EacR11unorm,
            wgpu_types::TextureFormat::EacR11Snorm => webgpu::GpuTextureFormat::EacR11snorm,
            wgpu_types::TextureFormat::EacRg11Unorm => webgpu::GpuTextureFormat::EacRg11unorm,
            wgpu_types::TextureFormat::EacRg11Snorm => webgpu::GpuTextureFormat::EacRg11snorm,
            wgpu_types::TextureFormat::Astc { .. } => todo!(),
            wgpu_types::TextureFormat::NV12 => todo!(),
            wgpu_types::TextureFormat::P010 => todo!(),
        }
    }
}

impl From<webgpu::GpuTextureFormat> for wgpu_types::TextureFormat {
    fn from(value: webgpu::GpuTextureFormat) -> Self {
        match value {
            webgpu::GpuTextureFormat::Bgra8unormSrgb => wgpu_types::TextureFormat::Bgra8UnormSrgb,
            webgpu::GpuTextureFormat::R8unorm => wgpu_types::TextureFormat::R8Unorm,
            webgpu::GpuTextureFormat::R8snorm => wgpu_types::TextureFormat::R8Snorm,
            webgpu::GpuTextureFormat::R8uint => wgpu_types::TextureFormat::R8Uint,
            webgpu::GpuTextureFormat::R8sint => wgpu_types::TextureFormat::R8Sint,
            webgpu::GpuTextureFormat::R16uint => wgpu_types::TextureFormat::R16Uint,
            webgpu::GpuTextureFormat::R16sint => wgpu_types::TextureFormat::R16Sint,
            webgpu::GpuTextureFormat::R16unorm => wgpu_types::TextureFormat::R16Unorm,
            webgpu::GpuTextureFormat::R16snorm => wgpu_types::TextureFormat::R16Snorm,
            webgpu::GpuTextureFormat::R16float => wgpu_types::TextureFormat::R16Float,
            webgpu::GpuTextureFormat::Rg8unorm => wgpu_types::TextureFormat::Rg8Unorm,
            webgpu::GpuTextureFormat::Rg8snorm => wgpu_types::TextureFormat::Rg8Snorm,
            webgpu::GpuTextureFormat::Rg8uint => wgpu_types::TextureFormat::Rg8Uint,
            webgpu::GpuTextureFormat::Rg8sint => wgpu_types::TextureFormat::Rg8Sint,
            webgpu::GpuTextureFormat::R32uint => wgpu_types::TextureFormat::R32Uint,
            webgpu::GpuTextureFormat::R32sint => wgpu_types::TextureFormat::R32Sint,
            webgpu::GpuTextureFormat::R32float => wgpu_types::TextureFormat::R32Float,
            webgpu::GpuTextureFormat::Rg16uint => wgpu_types::TextureFormat::Rg16Uint,
            webgpu::GpuTextureFormat::Rg16sint => wgpu_types::TextureFormat::Rg16Sint,
            webgpu::GpuTextureFormat::Rg16unorm => wgpu_types::TextureFormat::Rg16Unorm,
            webgpu::GpuTextureFormat::Rg16snorm => wgpu_types::TextureFormat::Rg16Snorm,
            webgpu::GpuTextureFormat::Rg16float => wgpu_types::TextureFormat::Rg16Float,
            webgpu::GpuTextureFormat::Rgba8unorm => wgpu_types::TextureFormat::Rgba8Unorm,
            webgpu::GpuTextureFormat::Rgba8unormSrgb => wgpu_types::TextureFormat::Rgba8UnormSrgb,
            webgpu::GpuTextureFormat::Rgba8snorm => wgpu_types::TextureFormat::Rgba8Snorm,
            webgpu::GpuTextureFormat::Rgba8uint => wgpu_types::TextureFormat::Rgba8Uint,
            webgpu::GpuTextureFormat::Rgba8sint => wgpu_types::TextureFormat::Rgba8Sint,
            webgpu::GpuTextureFormat::Bgra8unorm => wgpu_types::TextureFormat::Bgra8Unorm,
            webgpu::GpuTextureFormat::Rgb9e5ufloat => wgpu_types::TextureFormat::Rgb9e5Ufloat,
            webgpu::GpuTextureFormat::Rgb10a2uint => wgpu_types::TextureFormat::Rgb10a2Uint,
            webgpu::GpuTextureFormat::Rgb10a2unorm => wgpu_types::TextureFormat::Rgb10a2Unorm,
            webgpu::GpuTextureFormat::Rg11b10ufloat => wgpu_types::TextureFormat::Rg11b10Ufloat,
            webgpu::GpuTextureFormat::Rg32uint => wgpu_types::TextureFormat::Rg32Uint,
            webgpu::GpuTextureFormat::Rg32sint => wgpu_types::TextureFormat::Rg32Sint,
            webgpu::GpuTextureFormat::Rg32float => wgpu_types::TextureFormat::Rg32Float,
            webgpu::GpuTextureFormat::Rgba16uint => wgpu_types::TextureFormat::Rgba16Uint,
            webgpu::GpuTextureFormat::Rgba16sint => wgpu_types::TextureFormat::Rgba16Sint,
            webgpu::GpuTextureFormat::Rgba16unorm => wgpu_types::TextureFormat::Rgba16Unorm,
            webgpu::GpuTextureFormat::Rgba16snorm => wgpu_types::TextureFormat::Rgba16Snorm,
            webgpu::GpuTextureFormat::Rgba16float => wgpu_types::TextureFormat::Rgba16Float,
            webgpu::GpuTextureFormat::Rgba32uint => wgpu_types::TextureFormat::Rgba32Uint,
            webgpu::GpuTextureFormat::Rgba32sint => wgpu_types::TextureFormat::Rgba32Sint,
            webgpu::GpuTextureFormat::Rgba32float => wgpu_types::TextureFormat::Rgba32Float,
            webgpu::GpuTextureFormat::Stencil8 => wgpu_types::TextureFormat::Stencil8,
            webgpu::GpuTextureFormat::Depth16unorm => wgpu_types::TextureFormat::Depth16Unorm,
            webgpu::GpuTextureFormat::Depth24plus => wgpu_types::TextureFormat::Depth24Plus,
            webgpu::GpuTextureFormat::Depth24plusStencil8 => {
                wgpu_types::TextureFormat::Depth24PlusStencil8
            }
            webgpu::GpuTextureFormat::Depth32float => wgpu_types::TextureFormat::Depth32Float,
            webgpu::GpuTextureFormat::Depth32floatStencil8 => {
                wgpu_types::TextureFormat::Depth32FloatStencil8
            }
            webgpu::GpuTextureFormat::Bc1RgbaUnorm => wgpu_types::TextureFormat::Bc1RgbaUnorm,
            webgpu::GpuTextureFormat::Bc1RgbaUnormSrgb => {
                wgpu_types::TextureFormat::Bc1RgbaUnormSrgb
            }
            webgpu::GpuTextureFormat::Bc2RgbaUnorm => wgpu_types::TextureFormat::Bc2RgbaUnorm,
            webgpu::GpuTextureFormat::Bc2RgbaUnormSrgb => {
                wgpu_types::TextureFormat::Bc2RgbaUnormSrgb
            }
            webgpu::GpuTextureFormat::Bc3RgbaUnorm => wgpu_types::TextureFormat::Bc3RgbaUnorm,
            webgpu::GpuTextureFormat::Bc3RgbaUnormSrgb => {
                wgpu_types::TextureFormat::Bc3RgbaUnormSrgb
            }
            webgpu::GpuTextureFormat::Bc4RUnorm => wgpu_types::TextureFormat::Bc4RUnorm,
            webgpu::GpuTextureFormat::Bc4RSnorm => wgpu_types::TextureFormat::Bc4RSnorm,
            webgpu::GpuTextureFormat::Bc5RgUnorm => wgpu_types::TextureFormat::Bc5RgUnorm,
            webgpu::GpuTextureFormat::Bc5RgSnorm => wgpu_types::TextureFormat::Bc5RgSnorm,
            webgpu::GpuTextureFormat::Bc6hRgbUfloat => wgpu_types::TextureFormat::Bc6hRgbUfloat,
            webgpu::GpuTextureFormat::Bc6hRgbFloat => wgpu_types::TextureFormat::Bc6hRgbFloat,
            webgpu::GpuTextureFormat::Bc7RgbaUnorm => wgpu_types::TextureFormat::Bc7RgbaUnorm,
            webgpu::GpuTextureFormat::Bc7RgbaUnormSrgb => {
                wgpu_types::TextureFormat::Bc7RgbaUnormSrgb
            }
            webgpu::GpuTextureFormat::Etc2Rgb8unorm => wgpu_types::TextureFormat::Etc2Rgb8Unorm,
            webgpu::GpuTextureFormat::Etc2Rgb8unormSrgb => {
                wgpu_types::TextureFormat::Etc2Rgb8UnormSrgb
            }
            webgpu::GpuTextureFormat::Etc2Rgb8a1unorm => wgpu_types::TextureFormat::Etc2Rgb8A1Unorm,
            webgpu::GpuTextureFormat::Etc2Rgb8a1unormSrgb => {
                wgpu_types::TextureFormat::Etc2Rgb8A1UnormSrgb
            }
            webgpu::GpuTextureFormat::Etc2Rgba8unorm => wgpu_types::TextureFormat::Etc2Rgba8Unorm,
            webgpu::GpuTextureFormat::Etc2Rgba8unormSrgb => {
                wgpu_types::TextureFormat::Etc2Rgba8UnormSrgb
            }
            webgpu::GpuTextureFormat::EacR11unorm => wgpu_types::TextureFormat::EacR11Unorm,
            webgpu::GpuTextureFormat::EacR11snorm => wgpu_types::TextureFormat::EacR11Snorm,
            webgpu::GpuTextureFormat::EacRg11unorm => wgpu_types::TextureFormat::EacRg11Unorm,
            webgpu::GpuTextureFormat::EacRg11snorm => wgpu_types::TextureFormat::EacRg11Snorm,
            webgpu::GpuTextureFormat::Astc4x4Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc4x4UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc5x4Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc5x4UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc5x5Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc5x5UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc6x5Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc6x5UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc6x6Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc6x6UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc8x5Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc8x5UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc8x6Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc8x6UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc8x8Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc8x8UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc10x5Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc10x5UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc10x6Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc10x6UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc10x8Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc10x8UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc10x10Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc10x10UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc12x10Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc12x10UnormSrgb => todo!(),
            webgpu::GpuTextureFormat::Astc12x12Unorm => todo!(),
            webgpu::GpuTextureFormat::Astc12x12UnormSrgb => todo!(),
        }
    }
}

impl From<webgpu::GpuPrimitiveTopology> for wgpu_types::PrimitiveTopology {
    fn from(value: webgpu::GpuPrimitiveTopology) -> Self {
        match value {
            webgpu::GpuPrimitiveTopology::PointList => wgpu_types::PrimitiveTopology::PointList,
            webgpu::GpuPrimitiveTopology::LineList => wgpu_types::PrimitiveTopology::LineList,
            webgpu::GpuPrimitiveTopology::LineStrip => wgpu_types::PrimitiveTopology::LineStrip,
            webgpu::GpuPrimitiveTopology::TriangleList => {
                wgpu_types::PrimitiveTopology::TriangleList
            }
            webgpu::GpuPrimitiveTopology::TriangleStrip => {
                wgpu_types::PrimitiveTopology::TriangleStrip
            }
        }
    }
}

impl From<wgpu_types::PrimitiveTopology> for webgpu::GpuPrimitiveTopology {
    fn from(value: wgpu_types::PrimitiveTopology) -> Self {
        match value {
            wgpu_types::PrimitiveTopology::PointList => webgpu::GpuPrimitiveTopology::PointList,
            wgpu_types::PrimitiveTopology::LineList => webgpu::GpuPrimitiveTopology::LineList,
            wgpu_types::PrimitiveTopology::LineStrip => webgpu::GpuPrimitiveTopology::LineStrip,
            wgpu_types::PrimitiveTopology::TriangleList => {
                webgpu::GpuPrimitiveTopology::TriangleList
            }
            wgpu_types::PrimitiveTopology::TriangleStrip => {
                webgpu::GpuPrimitiveTopology::TriangleStrip
            }
        }
    }
}

impl From<webgpu::GpuTextureDimension> for wgpu_types::TextureDimension {
    fn from(value: webgpu::GpuTextureDimension) -> Self {
        match value {
            webgpu::GpuTextureDimension::D1 => wgpu_types::TextureDimension::D1,
            webgpu::GpuTextureDimension::D2 => wgpu_types::TextureDimension::D2,
            webgpu::GpuTextureDimension::D3 => wgpu_types::TextureDimension::D3,
        }
    }
}

impl From<webgpu::GpuAddressMode> for wgpu_types::AddressMode {
    fn from(value: webgpu::GpuAddressMode) -> Self {
        match value {
            webgpu::GpuAddressMode::ClampToEdge => wgpu_types::AddressMode::ClampToEdge,
            webgpu::GpuAddressMode::Repeat => wgpu_types::AddressMode::Repeat,
            webgpu::GpuAddressMode::MirrorRepeat => wgpu_types::AddressMode::MirrorRepeat,
        }
    }
}

impl From<webgpu::GpuFilterMode> for wgpu_types::FilterMode {
    fn from(value: webgpu::GpuFilterMode) -> Self {
        match value {
            webgpu::GpuFilterMode::Nearest => wgpu_types::FilterMode::Nearest,
            webgpu::GpuFilterMode::Linear => wgpu_types::FilterMode::Linear,
        }
    }
}

impl From<webgpu::GpuMipmapFilterMode> for wgpu_types::FilterMode {
    fn from(value: webgpu::GpuMipmapFilterMode) -> Self {
        match value {
            webgpu::GpuMipmapFilterMode::Nearest => wgpu_types::FilterMode::Nearest,
            webgpu::GpuMipmapFilterMode::Linear => wgpu_types::FilterMode::Linear,
        }
    }
}

// TODO: change to GpuMipmapFilterMode once in wasi:webgpu
impl From<webgpu::GpuFilterMode> for wgpu_types::MipmapFilterMode {
    fn from(value: webgpu::GpuFilterMode) -> Self {
        match value {
            webgpu::GpuFilterMode::Nearest => wgpu_types::MipmapFilterMode::Nearest,
            webgpu::GpuFilterMode::Linear => wgpu_types::MipmapFilterMode::Linear,
        }
    }
}

impl From<webgpu::GpuMipmapFilterMode> for wgpu_types::MipmapFilterMode {
    fn from(value: webgpu::GpuMipmapFilterMode) -> Self {
        match value {
            webgpu::GpuMipmapFilterMode::Nearest => wgpu_types::MipmapFilterMode::Nearest,
            webgpu::GpuMipmapFilterMode::Linear => wgpu_types::MipmapFilterMode::Linear,
        }
    }
}

impl From<webgpu::GpuCompareFunction> for wgpu_types::CompareFunction {
    fn from(value: webgpu::GpuCompareFunction) -> Self {
        match value {
            webgpu::GpuCompareFunction::Never => wgpu_types::CompareFunction::Never,
            webgpu::GpuCompareFunction::Less => wgpu_types::CompareFunction::Less,
            webgpu::GpuCompareFunction::Equal => wgpu_types::CompareFunction::Equal,
            webgpu::GpuCompareFunction::LessEqual => wgpu_types::CompareFunction::LessEqual,
            webgpu::GpuCompareFunction::Greater => wgpu_types::CompareFunction::Greater,
            webgpu::GpuCompareFunction::NotEqual => wgpu_types::CompareFunction::NotEqual,
            webgpu::GpuCompareFunction::GreaterEqual => wgpu_types::CompareFunction::GreaterEqual,
            webgpu::GpuCompareFunction::Always => wgpu_types::CompareFunction::Always,
        }
    }
}

impl From<webgpu::GpuSamplerBindingType> for wgpu_types::SamplerBindingType {
    fn from(value: webgpu::GpuSamplerBindingType) -> Self {
        match value {
            webgpu::GpuSamplerBindingType::Filtering => wgpu_types::SamplerBindingType::Filtering,
            webgpu::GpuSamplerBindingType::NonFiltering => {
                wgpu_types::SamplerBindingType::NonFiltering
            }
            webgpu::GpuSamplerBindingType::Comparison => wgpu_types::SamplerBindingType::Comparison,
        }
    }
}

impl From<webgpu::GpuTextureSampleType> for wgpu_types::TextureSampleType {
    fn from(value: webgpu::GpuTextureSampleType) -> Self {
        match value {
            webgpu::GpuTextureSampleType::Float => {
                wgpu_types::TextureSampleType::Float { filterable: true }
            }
            webgpu::GpuTextureSampleType::UnfilterableFloat => {
                wgpu_types::TextureSampleType::Float { filterable: false }
            }
            webgpu::GpuTextureSampleType::Depth => wgpu_types::TextureSampleType::Depth,
            webgpu::GpuTextureSampleType::Sint => wgpu_types::TextureSampleType::Sint,
            webgpu::GpuTextureSampleType::Uint => wgpu_types::TextureSampleType::Uint,
        }
    }
}

impl From<webgpu::GpuTextureViewDimension> for wgpu_types::TextureViewDimension {
    fn from(value: webgpu::GpuTextureViewDimension) -> Self {
        match value {
            webgpu::GpuTextureViewDimension::D1 => wgpu_types::TextureViewDimension::D1,
            webgpu::GpuTextureViewDimension::D2 => wgpu_types::TextureViewDimension::D2,
            webgpu::GpuTextureViewDimension::D2Array => wgpu_types::TextureViewDimension::D2Array,
            webgpu::GpuTextureViewDimension::Cube => wgpu_types::TextureViewDimension::Cube,
            webgpu::GpuTextureViewDimension::CubeArray => {
                wgpu_types::TextureViewDimension::CubeArray
            }
            webgpu::GpuTextureViewDimension::D3 => wgpu_types::TextureViewDimension::D3,
        }
    }
}

impl From<webgpu::GpuBufferBindingType> for wgpu_types::BufferBindingType {
    fn from(value: webgpu::GpuBufferBindingType) -> Self {
        match value {
            webgpu::GpuBufferBindingType::Uniform => wgpu_types::BufferBindingType::Uniform,
            webgpu::GpuBufferBindingType::Storage => {
                wgpu_types::BufferBindingType::Storage { read_only: false }
            }
            webgpu::GpuBufferBindingType::ReadOnlyStorage => {
                wgpu_types::BufferBindingType::Storage { read_only: true }
            }
        }
    }
}

impl From<webgpu::GpuStoreOp> for wgpu_core::command::StoreOp {
    fn from(value: webgpu::GpuStoreOp) -> Self {
        match value {
            webgpu::GpuStoreOp::Store => wgpu_core::command::StoreOp::Store,
            webgpu::GpuStoreOp::Discard => wgpu_core::command::StoreOp::Discard,
        }
    }
}

impl From<webgpu::GpuColor> for wgpu_types::Color {
    fn from(value: webgpu::GpuColor) -> Self {
        wgpu_types::Color {
            r: value.r,
            g: value.g,
            b: value.b,
            a: value.a,
        }
    }
}

impl From<webgpu::GpuCullMode> for Option<wgpu_types::Face> {
    fn from(value: webgpu::GpuCullMode) -> Self {
        match value {
            webgpu::GpuCullMode::None => None,
            webgpu::GpuCullMode::Front => Some(wgpu_types::Face::Front),
            webgpu::GpuCullMode::Back => Some(wgpu_types::Face::Back),
        }
    }
}

impl From<webgpu::GpuIndexFormat> for wgpu_types::IndexFormat {
    fn from(value: webgpu::GpuIndexFormat) -> Self {
        match value {
            webgpu::GpuIndexFormat::Uint16 => wgpu_types::IndexFormat::Uint16,
            webgpu::GpuIndexFormat::Uint32 => wgpu_types::IndexFormat::Uint32,
        }
    }
}

impl From<webgpu::GpuFrontFace> for wgpu_types::FrontFace {
    fn from(value: webgpu::GpuFrontFace) -> Self {
        match value {
            webgpu::GpuFrontFace::Ccw => wgpu_types::FrontFace::Ccw,
            webgpu::GpuFrontFace::Cw => wgpu_types::FrontFace::Cw,
        }
    }
}

impl From<webgpu::GpuVertexStepMode> for wgpu_types::VertexStepMode {
    fn from(value: webgpu::GpuVertexStepMode) -> Self {
        match value {
            webgpu::GpuVertexStepMode::Vertex => wgpu_types::VertexStepMode::Vertex,
            webgpu::GpuVertexStepMode::Instance => wgpu_types::VertexStepMode::Instance,
        }
    }
}

impl From<webgpu::GpuVertexFormat> for wgpu_types::VertexFormat {
    fn from(value: webgpu::GpuVertexFormat) -> Self {
        match value {
            webgpu::GpuVertexFormat::Uint8 => todo!(),
            webgpu::GpuVertexFormat::Uint8x2 => wgpu_types::VertexFormat::Uint8x2,
            webgpu::GpuVertexFormat::Uint8x4 => wgpu_types::VertexFormat::Uint8x4,
            webgpu::GpuVertexFormat::Sint8 => todo!(),
            webgpu::GpuVertexFormat::Sint8x2 => wgpu_types::VertexFormat::Sint8x2,
            webgpu::GpuVertexFormat::Sint8x4 => wgpu_types::VertexFormat::Sint8x4,
            webgpu::GpuVertexFormat::Unorm8 => todo!(),
            webgpu::GpuVertexFormat::Unorm8x2 => wgpu_types::VertexFormat::Unorm8x2,
            webgpu::GpuVertexFormat::Unorm8x4 => wgpu_types::VertexFormat::Unorm8x4,
            webgpu::GpuVertexFormat::Snorm8 => todo!(),
            webgpu::GpuVertexFormat::Snorm8x2 => wgpu_types::VertexFormat::Snorm8x2,
            webgpu::GpuVertexFormat::Snorm8x4 => wgpu_types::VertexFormat::Snorm8x4,
            webgpu::GpuVertexFormat::Uint16 => todo!(),
            webgpu::GpuVertexFormat::Uint16x2 => wgpu_types::VertexFormat::Uint16x2,
            webgpu::GpuVertexFormat::Uint16x4 => wgpu_types::VertexFormat::Uint16x4,
            webgpu::GpuVertexFormat::Sint16 => todo!(),
            webgpu::GpuVertexFormat::Sint16x2 => wgpu_types::VertexFormat::Sint16x2,
            webgpu::GpuVertexFormat::Sint16x4 => wgpu_types::VertexFormat::Sint16x4,
            webgpu::GpuVertexFormat::Unorm16 => todo!(),
            webgpu::GpuVertexFormat::Unorm16x2 => wgpu_types::VertexFormat::Unorm16x2,
            webgpu::GpuVertexFormat::Unorm16x4 => wgpu_types::VertexFormat::Unorm16x4,
            webgpu::GpuVertexFormat::Snorm16 => todo!(),
            webgpu::GpuVertexFormat::Snorm16x2 => wgpu_types::VertexFormat::Snorm16x2,
            webgpu::GpuVertexFormat::Snorm16x4 => wgpu_types::VertexFormat::Snorm16x4,
            webgpu::GpuVertexFormat::Float16 => todo!(),
            webgpu::GpuVertexFormat::Float16x2 => wgpu_types::VertexFormat::Float16x2,
            webgpu::GpuVertexFormat::Float16x4 => wgpu_types::VertexFormat::Float16x4,
            webgpu::GpuVertexFormat::Float32 => wgpu_types::VertexFormat::Float32,
            webgpu::GpuVertexFormat::Float32x2 => wgpu_types::VertexFormat::Float32x2,
            webgpu::GpuVertexFormat::Float32x3 => wgpu_types::VertexFormat::Float32x3,
            webgpu::GpuVertexFormat::Float32x4 => wgpu_types::VertexFormat::Float32x4,
            webgpu::GpuVertexFormat::Uint32 => wgpu_types::VertexFormat::Uint32,
            webgpu::GpuVertexFormat::Uint32x2 => wgpu_types::VertexFormat::Uint32x2,
            webgpu::GpuVertexFormat::Uint32x3 => wgpu_types::VertexFormat::Uint32x3,
            webgpu::GpuVertexFormat::Uint32x4 => wgpu_types::VertexFormat::Uint32x4,
            webgpu::GpuVertexFormat::Sint32 => wgpu_types::VertexFormat::Sint32,
            webgpu::GpuVertexFormat::Sint32x2 => wgpu_types::VertexFormat::Sint32x2,
            webgpu::GpuVertexFormat::Sint32x3 => wgpu_types::VertexFormat::Sint32x3,
            webgpu::GpuVertexFormat::Sint32x4 => wgpu_types::VertexFormat::Sint32x4,
            webgpu::GpuVertexFormat::Unorm1010102 => wgpu_types::VertexFormat::Unorm10_10_10_2,
            webgpu::GpuVertexFormat::Unorm8x4Bgra => todo!(),
        }
    }
}

impl From<webgpu::GpuBlendFactor> for wgpu_types::BlendFactor {
    fn from(value: webgpu::GpuBlendFactor) -> Self {
        match value {
            webgpu::GpuBlendFactor::Zero => wgpu_types::BlendFactor::Zero,
            webgpu::GpuBlendFactor::One => wgpu_types::BlendFactor::One,
            webgpu::GpuBlendFactor::Src => wgpu_types::BlendFactor::Src,
            webgpu::GpuBlendFactor::OneMinusSrc => wgpu_types::BlendFactor::OneMinusSrc,
            webgpu::GpuBlendFactor::SrcAlpha => wgpu_types::BlendFactor::SrcAlpha,
            webgpu::GpuBlendFactor::OneMinusSrcAlpha => wgpu_types::BlendFactor::OneMinusSrcAlpha,
            webgpu::GpuBlendFactor::Dst => wgpu_types::BlendFactor::Dst,
            webgpu::GpuBlendFactor::OneMinusDst => wgpu_types::BlendFactor::OneMinusDst,
            webgpu::GpuBlendFactor::DstAlpha => wgpu_types::BlendFactor::DstAlpha,
            webgpu::GpuBlendFactor::OneMinusDstAlpha => wgpu_types::BlendFactor::OneMinusDstAlpha,
            webgpu::GpuBlendFactor::SrcAlphaSaturated => wgpu_types::BlendFactor::SrcAlphaSaturated,
            webgpu::GpuBlendFactor::Constant => wgpu_types::BlendFactor::Constant,
            webgpu::GpuBlendFactor::OneMinusConstant => wgpu_types::BlendFactor::OneMinusConstant,
            webgpu::GpuBlendFactor::Src1 => wgpu_types::BlendFactor::Src1,
            webgpu::GpuBlendFactor::OneMinusSrc1 => wgpu_types::BlendFactor::OneMinusSrc1,
            webgpu::GpuBlendFactor::Src1Alpha => wgpu_types::BlendFactor::Src1Alpha,
            webgpu::GpuBlendFactor::OneMinusSrc1Alpha => wgpu_types::BlendFactor::OneMinusSrc1Alpha,
        }
    }
}

impl From<webgpu::GpuBlendOperation> for wgpu_types::BlendOperation {
    fn from(value: webgpu::GpuBlendOperation) -> Self {
        match value {
            webgpu::GpuBlendOperation::Add => wgpu_types::BlendOperation::Add,
            webgpu::GpuBlendOperation::Subtract => wgpu_types::BlendOperation::Subtract,
            webgpu::GpuBlendOperation::ReverseSubtract => {
                wgpu_types::BlendOperation::ReverseSubtract
            }
            webgpu::GpuBlendOperation::Min => wgpu_types::BlendOperation::Min,
            webgpu::GpuBlendOperation::Max => wgpu_types::BlendOperation::Max,
        }
    }
}

impl From<webgpu::GpuStencilOperation> for wgpu_types::StencilOperation {
    fn from(value: webgpu::GpuStencilOperation) -> Self {
        match value {
            webgpu::GpuStencilOperation::Keep => wgpu_types::StencilOperation::Keep,
            webgpu::GpuStencilOperation::Zero => wgpu_types::StencilOperation::Zero,
            webgpu::GpuStencilOperation::Replace => wgpu_types::StencilOperation::Replace,
            webgpu::GpuStencilOperation::Invert => wgpu_types::StencilOperation::Invert,
            webgpu::GpuStencilOperation::IncrementClamp => {
                wgpu_types::StencilOperation::IncrementClamp
            }
            webgpu::GpuStencilOperation::DecrementClamp => {
                wgpu_types::StencilOperation::DecrementClamp
            }
            webgpu::GpuStencilOperation::IncrementWrap => {
                wgpu_types::StencilOperation::IncrementWrap
            }
            webgpu::GpuStencilOperation::DecrementWrap => {
                wgpu_types::StencilOperation::DecrementWrap
            }
        }
    }
}

impl From<webgpu::GpuTextureAspect> for wgpu_types::TextureAspect {
    fn from(value: webgpu::GpuTextureAspect) -> Self {
        match value {
            webgpu::GpuTextureAspect::All => wgpu_types::TextureAspect::All,
            webgpu::GpuTextureAspect::StencilOnly => wgpu_types::TextureAspect::StencilOnly,
            webgpu::GpuTextureAspect::DepthOnly => wgpu_types::TextureAspect::DepthOnly,
        }
    }
}

impl From<wgpu_types::error::ErrorType> for webgpu::GpuErrorKind {
    fn from(value: wgpu_types::error::ErrorType) -> Self {
        match value {
            wgpu_types::error::ErrorType::Internal => webgpu::GpuErrorKind::InternalError,
            wgpu_types::error::ErrorType::OutOfMemory => webgpu::GpuErrorKind::OutOfMemoryError,
            wgpu_types::error::ErrorType::Validation => webgpu::GpuErrorKind::ValidationError,
            wgpu_types::error::ErrorType::DeviceLost => todo!(),
        }
    }
}

impl From<webgpu::GpuCanvasAlphaMode> for wgpu_types::CompositeAlphaMode {
    fn from(value: webgpu::GpuCanvasAlphaMode) -> Self {
        match value {
            webgpu::GpuCanvasAlphaMode::Opaque => wgpu_types::CompositeAlphaMode::Opaque,
            webgpu::GpuCanvasAlphaMode::Premultiplied => {
                wgpu_types::CompositeAlphaMode::PreMultiplied
            }
        }
    }
}
