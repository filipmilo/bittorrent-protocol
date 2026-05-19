use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};

#[derive(Debug)]
pub struct FileSerializer {
    block_size: u64,
    file: File,
}

impl FileSerializer {
    pub fn new(
        file_name: &String,
        block_size: u64,
        total_size: u64,
    ) -> Result<Self, std::io::Error> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(file_name)?;

        /*
         * This sets the file's logical size without allocating disk blocks
         * for the zero regions — the OS marks those as holes.
         */
        file.seek(SeekFrom::Start(total_size - 1))?;
        file.write_all(&[0])?;

        Ok(Self { file, block_size })
    }

    pub fn save_piece(&mut self, index: u64, piece: Vec<u8>) -> Result<(), std::io::Error> {
        self.file.seek(SeekFrom::Start(index * self.block_size))?;

        self.file.write_all(&piece)?;

        Ok(())
    }
}

