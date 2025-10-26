/// `#[repr(C)]` ensures Rust doesn't reorder the fields.
/// `Pod` is a bytemuck trait meaning "Plain Old Data".
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AudioMetadata {
    pub sample_rate: f32,
    pub n_channels: u32,
    pub n_samples_per_channel: u32,
}

/// We can also define a constant for the size.
pub const METADATA_SIZE: usize = std::mem::size_of::<AudioMetadata>();
