use crate::DepotFile;

#[derive(Clone)]
pub enum FileType {
    DepotFile,
    Other,
}

#[derive(Clone)]
pub struct DownloadUnit {
    pub md5: String,
    pub size: u64,
    pub compressed_md5: String,
    pub _compressed_size: u64,
    pub path: String,
    pub offset: u64,
    pub file_type: FileType,
}

impl DownloadUnit {
    pub fn from_depot_file(depot_file: DepotFile) -> Vec<DownloadUnit> {
        let mut offset = 0;
        let mut units: Vec<DownloadUnit> = Vec::new();

        if let Some(chunks) = depot_file.chunks {
            for chunk in chunks.iter() {
                units.push(DownloadUnit {
                    md5: chunk.md5.clone(),
                    size: chunk.size,
                    compressed_md5: chunk.compressed_md5.clone(),
                    _compressed_size: chunk.compressed_size,
                    path: depot_file.path.clone(),
                    offset: offset,
                    file_type: match depot_file.file_type.as_ref() {
                        "DepotFile" => FileType::DepotFile,
                        _ => FileType::Other,
                    },
                });
                offset += chunk.size
            }
        }
        units
    }
}
