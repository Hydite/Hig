use crate::Compression;
use std::io::Read;

pub fn compress(codec: Compression, input: &[u8], level: i32) -> anyhow::Result<Vec<u8>> {
    match codec {
        Compression::Zstd => Ok(zstd::bulk::compress(input, level)?),
    }
}

pub fn decompress(codec: Compression, input: &[u8], original_size: u64) -> anyhow::Result<Vec<u8>> {
    let original_size = usize::try_from(original_size)
        .map_err(|_| anyhow::anyhow!("decompressed size does not fit this platform"))?;
    match codec {
        Compression::Zstd => Ok(zstd::bulk::decompress(input, original_size)?),
    }
}

pub fn decompress_unknown_bounded(
    codec: Compression,
    input: &[u8],
    maximum_output_bytes: u64,
) -> anyhow::Result<Vec<u8>> {
    match codec {
        Compression::Zstd => {
            let decoder = zstd::stream::read::Decoder::new(input)?;
            let mut limited = decoder.take(maximum_output_bytes.saturating_add(1));
            let mut output = Vec::new();
            limited.read_to_end(&mut output)?;
            anyhow::ensure!(
                output.len() as u64 <= maximum_output_bytes,
                "decompressed payload exceeds resource limit"
            );
            Ok(output)
        }
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
