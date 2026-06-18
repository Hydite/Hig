use crate::Compression;

pub fn compress(codec: Compression, input: &[u8], level: i32) -> anyhow::Result<Vec<u8>> {
    match codec {
        Compression::Zstd => Ok(zstd::bulk::compress(input, level)?),
    }
}

pub fn decompress(codec: Compression, input: &[u8], original_size: u64) -> anyhow::Result<Vec<u8>> {
    match codec {
        Compression::Zstd => Ok(zstd::bulk::decompress(input, original_size as usize)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_roundtrip() {
        let data = b"hello hig ".repeat(1024);
        let compressed = compress(Compression::Zstd, &data, 1).unwrap();
        let decompressed = decompress(Compression::Zstd, &compressed, data.len() as u64).unwrap();
        assert_eq!(decompressed, data);
    }
}
