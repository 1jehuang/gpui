use super::{PATH_RECORD_SIZE, PATH_STORAGE_STRIDE, path_records};

#[test]
fn path_records_preserve_payload_and_zero_each_storage_tail() {
    let bytes: Vec<u8> = (0..PATH_RECORD_SIZE * 3).map(|i| i as u8).collect();
    let padded = path_records(&bytes);
    assert_eq!(padded.len(), PATH_STORAGE_STRIDE * 3);
    assert_eq!(PATH_STORAGE_STRIDE % 16, 0);
    for (input, output) in bytes
        .chunks_exact(PATH_RECORD_SIZE)
        .zip(padded.chunks_exact(PATH_STORAGE_STRIDE))
    {
        assert_eq!(&output[..PATH_RECORD_SIZE], input);
        assert!(output[PATH_RECORD_SIZE..].iter().all(|byte| *byte == 0));
    }
    assert!(path_records(&[]).is_empty());
}

#[test]
fn storage_shader_path_array_stride_matches_encoded_records() {
    let source = concat!(
        include_str!("../../shaders.wgsl"),
        include_str!("../shaders_storage.wgsl")
    );
    let module = naga::front::wgsl::parse_str(source).unwrap();
    let mut found = false;
    for (_, ty) in module.types.iter() {
        if let naga::TypeInner::Array { base, stride, .. } = ty.inner {
            if module.types[base].name.as_deref() == Some("StoragePathVertex") {
                assert_eq!(stride as usize, PATH_STORAGE_STRIDE);
                found = true;
            }
        }
    }
    assert!(found);
    for (_, global) in module.global_variables.iter() {
        if global.binding.is_some() {
            if let naga::TypeInner::Array { stride, .. } = module.types[global.ty].inner {
                assert_eq!(
                    stride % 16,
                    0,
                    "storage binding {:?} is not GLES aligned",
                    global.name
                );
            }
        }
    }
}
