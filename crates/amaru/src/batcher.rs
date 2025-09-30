use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use async_compression::futures::write::GzipEncoder;
use async_compression::Level;
use tar::Builder;

pub struct Batcher {
    network: String,
    blocks: Vec<(String, Vec<u8>)>, // (filename, contents)
    counter: usize,
}

impl Batcher {
    pub fn new(network: String) -> Self {
        Self {
            network,
            blocks: Vec::new(),
            counter: 0,
        }
    }

    pub async fn write_block(&mut self, slot: u64, raw_block: Vec<u8>) {
        let filename = format!("{}.cbor", slot);
        self.blocks.push((filename, raw_block));
        self.counter += 1;

        if self.counter % 1000 == 0 {
            if let Err(e) = self.package_blocks().await {
                tracing::error!("Failed to package blocks: {}", e);
            }
            self.blocks.clear(); // reset buffer
        }
    }

    async fn package_blocks(&self) -> std::io::Result<()> {
        let dir = format!("data/{}/blocks", self.network);
        tokio::fs::create_dir_all(&dir).await?;

        let archive_path = format!("{}/batch-{}.tar.gz", dir, self.counter);

        let file = File::create(&archive_path).await?;
        let writer = BufWriter::new(file);
        let enc = GzipEncoder::with_quality(writer, Level::Default);

        // tar::Builder is sync, but fine for ~1000 blocks
        let mut tar = Builder::new(enc);

        for (name, contents) in &self.blocks {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_cksum();
            header.set_mode(0o644);
            header.set_mtime(0);
            tar.append_data(&mut header, name.as_str(), &contents[..])?;
        }

        // Finalize tar → gzip → file
        let enc = tar.into_inner()?;          // get GzipEncoder back
        let mut writer = enc.shutdown().await?; // flush compression
        writer.flush().await?;
        Ok(())
    }
}