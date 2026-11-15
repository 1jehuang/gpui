//! GLES storage blocks require a 16-byte minimum size on downlevel adapters.
//! Keep the shared/texture path ABI at 104 bytes, but encode storage records
//! with an explicit zero-filled tail matching `StoragePathVertex` in WGSL.

pub(crate) const PATH_RECORD_SIZE: usize = 104;
pub(crate) const PATH_STORAGE_STRIDE: usize = 112;

pub(crate) fn path_records(data: &[u8]) -> Vec<u8> {
    assert_eq!(data.len() % PATH_RECORD_SIZE, 0);
    let mut padded = Vec::with_capacity(data.len() / PATH_RECORD_SIZE * PATH_STORAGE_STRIDE);
    for record in data.chunks_exact(PATH_RECORD_SIZE) {
        padded.extend_from_slice(record);
        padded.resize(padded.len() + PATH_STORAGE_STRIDE - PATH_RECORD_SIZE, 0);
    }
    padded
}

#[cfg(test)]
#[path = "tests/storage_padding.rs"]
mod tests;
