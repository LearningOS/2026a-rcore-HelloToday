use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    inode_id: u32,
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// Create a vfs inode
    pub fn new(
        inode_id: u32,
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
    ) -> Self {
        Self {
            inode_id,
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
        }
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }

    ///在目录中查找指定名称，返回目录项下标和inode编号
    fn find_dirent(&self, name: &str, disk_inode: &DiskInode) -> Option<(usize, u32)> {
        assert!(disk_inode.is_dir());

        let file_count = disk_inode.size as usize / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for index in 0..file_count {
            assert_eq!(disk_inode.read_at(index*DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device), DIRENT_SZ);
            if dirent.name() == name {
                return Some((index, dirent.inode_id()));
            }
        }
        None
    }

    ///在目录中查找指定名称，只返回inode编号
    fn find_inode_id(&self, name: &str,disk_inode: &DiskInode) -> Option<u32> {
        self.find_dirent(name, disk_inode).map(|(_, inode_id)| inode_id)
    }

    ///在目录中查找可以服用的空目录项
    fn find_empty_dirent(&self, disk_inode: &DiskInode) -> Option<usize> {
        assert!(disk_inode.is_dir());
        let file_count = disk_inode.size as usize / DIRENT_SZ;
        let mut dirent = DirEntry::empty();

        for index in 0..file_count {
            assert_eq!(disk_inode.read_at(index*DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device), DIRENT_SZ);
            if dirent.name().is_empty() {
                return Some(index);
            }
        }
        None
    }


    ///写入一个名称到inode的映射，优先复用空目录项
    fn write_dirent(&self, name: &str, inode_id: u32, root_inode: &mut DiskInode, fs: &mut MutexGuard<EasyFileSystem>)
    {
        assert!(root_inode.is_dir());

        let file_count = root_inode.size as usize / DIRENT_SZ;

        let offset = if let Some(index) = self.find_empty_dirent(root_inode)
        {
            index*DIRENT_SZ
        }
        else
        {
            let new_size = (file_count + 1) * DIRENT_SZ;
            self.increase_size(new_size as u32, root_inode, fs);
            file_count*DIRENT_SZ
        };
        let dirent = DirEntry::new(name, inode_id);
        assert_eq!(root_inode.write_at(offset, dirent.as_bytes(),&self.block_device),DIRENT_SZ);
    }


    ///返回文件系统中的逻辑inode编号
    pub fn inode_id(&self) -> u32 {
        self.inode_id
    }

    ///返回当前inode的硬链接数量
    pub fn nlink(&self) -> u32 {
        self.read_disk_inode(|disk_inode| disk_inode.nlink())
    }

    ///判断当前inode是否为目录
    pub fn is_dir(&self) -> bool {
        self.read_disk_inode(|disk_inode| disk_inode.is_dir())
    }




    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    inode_id,
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                ))
            })
        })
    }
    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.modify_disk_inode(|root_inode| {
            self.write_dirent(name, new_inode_id, root_inode, &mut fs);
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            new_inode_id,
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
        )))
        // release efs lock automatically by compiler
    }

    ///为已有文件创建一个新的硬链接
    pub fn link(&self, old_name:&str, new_name:&str) -> bool {
        if old_name.is_empty() || new_name.is_empty() || old_name == new_name {
            return  false;
        }

        let mut fs = self.fs.lock();
        let old_inode_id = match self.read_disk_inode(|root_inode| {
            assert!(root_inode.is_dir());
            if self.find_inode_id(new_name, root_inode).is_some()
            {
                None
            }
            else {
                self.find_inode_id(old_name, root_inode)
            }
        
        }) {
            Some(inode_id) => inode_id,
            None => return false,
        };

        let (inode_block_id, inode_block_offset) = fs.get_disk_inode_pos(old_inode_id);

        get_block_cache(inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(
                inode_block_offset,
                |disk_inode: &mut DiskInode| {
                    disk_inode.increase_nlink();
                },
            );

        self.modify_disk_inode(|root_inode| {
            self.write_dirent(new_name, old_inode_id, root_inode, &mut fs);
        });
        block_cache_sync_all();  
        

        true
    }

    ///删除目录项，并减少对应inode的硬链接数量
    pub fn unlink(&self, name: &str) -> bool {
        if name.is_empty() {
            return  false;
        }
        let mut fs = self.fs.lock();

        let (dirent_index, inode_id) = 
            match self.read_disk_inode(|root_inode| {
                self.find_dirent(name, root_inode)
            }) {
                Some(result) => result,
                None => return false
            };
        self.modify_disk_inode(|root_inode| {
            let empty_dirent = DirEntry::empty();

            assert_eq!(
                root_inode.write_at(dirent_index*DIRENT_SZ, empty_dirent.as_bytes(), &self.block_device),
                DIRENT_SZ
            );
        });
        let (inode_block_id, inode_block_offset) = fs.get_disk_inode_pos(inode_id);

        let (remaining_nlink, data_blocks) = get_block_cache(inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(
                inode_block_offset,
                |disk_inode: &mut DiskInode| {
                    let remaining_nlink = disk_inode.decrease_nlink();

                    let data_blocks = if remaining_nlink == 0 {
                        let old_size = disk_inode.size;
                        let data_blocks = disk_inode.clear_size(&self.block_device);

                        assert_eq!(data_blocks.len(), DiskInode::total_blocks(old_size) as usize);

                        data_blocks
                    } else {
                        Vec::new()
                    
                    };
                    (remaining_nlink, data_blocks)
                }
            );
        if remaining_nlink == 0 {
            for data_block in data_blocks {
                fs.dealloc_data(data_block);
            }

            fs.dealloc_inode(inode_id);
        }

        block_cache_sync_all();
        true
    }


    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                let name = dirent.name();
                if !name.is_empty() {
                    v.push(String::from(name));
                }
                
            }
            v
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
}
