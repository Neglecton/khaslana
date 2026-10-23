//! 安全边界在创建时建立，不能先用默认 ACL 写入私钥后再收紧。
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use super::{Result, UNSAFE, error};

pub(super) struct Directory {
    pub(super) path: PathBuf,
    handle: File,
    _ancestors: Vec<Directory>,
}

impl Directory {
    pub(super) fn open_tree(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        let mut prefix = PathBuf::new();
        let mut directories = Vec::new();
        // 所有现存祖先均拒绝 symlink/junction；保留句柄防止 Windows 祖先被换名。
        for component in path.components() {
            if matches!(component, Component::ParentDir) { return Err(error(UNSAFE)); }
            prefix.push(component);
            if matches!(component, Component::Prefix(_)) { continue; }
            let secure = prefix == path || prefix == path.parent().unwrap_or(&path);
            let directory = platform::open_tree_step(directories.last(), &prefix, secure)?;
            directories.push(directory);
        }
        let mut directory = directories.pop().ok_or_else(|| error(UNSAFE))?;
        directory._ancestors = directories;
        Ok(directory)
    }

    pub(super) fn create_child(&self, name: &str) -> Result<Self> {
        platform::child(self, name, true)
    }
    pub(super) fn open_child(&self, name: &str) -> Result<Self> {
        platform::child(self, name, false)
    }
    pub(super) fn write_new(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let mut file = platform::file(self, name, true)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    pub(super) fn open_file(&self, name: &str) -> Result<File> { platform::file(self, name, false) }
    pub(super) fn remove_file(&self, name: &str) -> Result<()> { platform::remove(self, name, false) }
    pub(super) fn remove_child(&self, name: &str) -> Result<()> { platform::remove(self, name, true) }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::{ffi::OsStrExt, fs::{MetadataExt, PermissionsExt}, io::{AsRawFd, FromRawFd}};

    fn cpath(path: &Path) -> Result<CString> { CString::new(path.as_os_str().as_bytes()).map_err(|_| error(UNSAFE)) }
    fn checked(fd: i32, path: PathBuf, secure: bool) -> Result<Directory> {
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        let handle = unsafe { File::from_raw_fd(fd) };
        let metadata = handle.metadata()?;
        if secure && (metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0) {
            return Err(error(UNSAFE));
        }
        Ok(Directory { path, handle, _ancestors: Vec::new() })
    }
    pub(super) fn open_tree_step(parent: Option<&Directory>, path: &Path, secure: bool) -> Result<Directory> {
        if let Some(parent) = parent {
            let name = cpath(Path::new(path.file_name().ok_or_else(|| error(UNSAFE))?))?;
            if secure && unsafe { libc::mkdirat(parent.handle.as_raw_fd(), name.as_ptr(), 0o700) } < 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(std::io::Error::last_os_error().into());
            }
            return checked(unsafe { libc::openat(parent.handle.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) }, path.into(), secure);
        }
        open_directory(path, secure, false)
    }
    pub(super) fn open_directory(path: &Path, secure: bool, exclusive: bool) -> Result<Directory> {
        let name = cpath(path)?;
        if secure {
            let created = unsafe { libc::mkdir(name.as_ptr(), 0o700) };
            if created < 0 && (exclusive || std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)) {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        checked(unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) }, path.into(), secure)
    }
    pub(super) fn child(parent: &Directory, name: &str, create: bool) -> Result<Directory> {
        let name_c = cpath(Path::new(name))?;
        if create && unsafe { libc::mkdirat(parent.handle.as_raw_fd(), name_c.as_ptr(), 0o700) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        checked(unsafe { libc::openat(parent.handle.as_raw_fd(), name_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) }, parent.path.join(name), true)
    }
    pub(super) fn file(parent: &Directory, name: &str, create: bool) -> Result<File> {
        let name = cpath(Path::new(name))?;
        let flags = libc::O_NOFOLLOW | libc::O_CLOEXEC | if create { libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL } else { libc::O_RDONLY };
        let fd = unsafe { libc::openat(parent.handle.as_raw_fd(), name.as_ptr(), flags, 0o600) };
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() } || metadata.permissions().mode() & 0o077 != 0 {
            return Err(error(UNSAFE));
        }
        Ok(file)
    }
    pub(super) fn remove(parent: &Directory, name: &str, directory: bool) -> Result<()> {
        let name = cpath(Path::new(name))?;
        if unsafe { libc::unlinkat(parent.handle.as_raw_fd(), name.as_ptr(), if directory { libc::AT_REMOVEDIR } else { 0 }) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::{ffi::OsStrExt, fs::MetadataExt, io::{FromRawHandle, AsRawHandle}};
    use windows_sys::Win32::{Foundation::*, Security::*, Security::Authorization::*, Storage::FileSystem::*, System::Threading::*};

    fn wide(path: &Path) -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() }
    struct Descriptor(PSECURITY_DESCRIPTOR);
    impl Drop for Descriptor { fn drop(&mut self) { unsafe { LocalFree(self.0); } } }
    fn descriptor() -> Result<Descriptor> {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 { return Err(std::io::Error::last_os_error().into()); }
            let mut length = 0;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length);
            // Vec<usize> 保证 TOKEN_USER 对齐；不把 token 或 SID 写入日志。
            let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
            let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), length, &mut length);
            CloseHandle(token);
            if ok == 0 { return Err(std::io::Error::last_os_error().into()); }
            let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
            let mut sid = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 { return Err(std::io::Error::last_os_error().into()); }
            let mut count = 0;
            while *sid.add(count) != 0 { count += 1; }
            let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, count));
            LocalFree(sid.cast());
            // 受保护 DACL：只有当前用户一条 ACE，无 Everyone、Users 或继承条目。
            let sddl = wide(Path::new(&format!("O:{sid_text}D:P(A;OICI;FA;;;{sid_text})")));
            let mut descriptor = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()) == 0 { return Err(std::io::Error::last_os_error().into()); }
            Ok(Descriptor(descriptor))
        }
    }
    fn attributes(descriptor: &Descriptor) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor.0, bInheritHandle: 0 }
    }
    fn verify_acl(file: &File) -> Result<()> {
        unsafe {
            let mut actual = std::ptr::null_mut();
            let mut owner = std::ptr::null_mut();
            let mut acl = std::ptr::null_mut();
            let code = GetSecurityInfo(file.as_raw_handle() as _, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut owner, std::ptr::null_mut(), &mut acl, std::ptr::null_mut(), &mut actual);
            if code != 0 { return Err(error(UNSAFE)); }
            let actual = Descriptor(actual);
            let expected = descriptor()?;
            let mut expected_owner = std::ptr::null_mut();
            let mut defaulted = 0;
            GetSecurityDescriptorOwner(expected.0, &mut expected_owner, &mut defaulted);
            let mut control = 0;
            let mut revision = 0;
            let mut ace: *mut c_void = std::ptr::null_mut();
            let valid = GetSecurityDescriptorControl(actual.0, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED != 0 && !owner.is_null()
                && EqualSid(owner, expected_owner) != 0 && !acl.is_null()
                && (*acl).AceCount == 1 && GetAce(acl, 0, &mut ace) != 0;
            if !valid { return Err(error(UNSAFE)); }
            let ace = &*(ace.cast::<ACCESS_ALLOWED_ACE>());
            let sid = (&ace.SidStart as *const u32).cast_mut().cast();
            // ACCESS_ALLOWED_ACE_TYPE 在 Win32 中固定为 0。
            if ace.Header.AceType != 0 || u32::from(ace.Header.AceFlags) & INHERITED_ACE != 0
                || ace.Mask & FILE_ALL_ACCESS != FILE_ALL_ACCESS || EqualSid(sid, expected_owner) == 0 { return Err(error(UNSAFE)); }
            Ok(())
        }
    }
    fn open(path: &Path, create: bool, directory: bool, secure: bool) -> Result<File> {
        let descriptor = descriptor()?;
        let attributes = attributes(&descriptor);
        let handle = unsafe { CreateFileW(wide(path).as_ptr(), if create { GENERIC_READ | GENERIC_WRITE } else { GENERIC_READ }, FILE_SHARE_READ | FILE_SHARE_WRITE, &attributes, if create { CREATE_NEW } else { OPEN_EXISTING }, FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 }, std::ptr::null_mut()) };
        if handle == INVALID_HANDLE_VALUE { return Err(std::io::Error::last_os_error().into()); }
        let file = unsafe { File::from_raw_handle(handle.cast()) };
        let metadata = file.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || metadata.is_dir() != directory { return Err(error(UNSAFE)); }
        if !directory {
            let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
            if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0
                || information.nNumberOfLinks != 1 { return Err(error(UNSAFE)); }
        }
        if secure { verify_acl(&file)?; }
        Ok(file)
    }
    pub(super) fn open_tree_step(_parent: Option<&Directory>, path: &Path, secure: bool) -> Result<Directory> {
        open_directory(path, secure, false)
    }
    pub(super) fn open_directory(path: &Path, secure: bool, exclusive: bool) -> Result<Directory> {
        if secure {
            let descriptor = descriptor()?;
            let attributes = attributes(&descriptor);
            if unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) } == 0 {
                let err = std::io::Error::last_os_error();
                if exclusive || err.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) { return Err(err.into()); }
            }
        }
        let handle = open(path, false, true, secure)?;
        Ok(Directory { path: path.into(), handle, _ancestors: Vec::new() })
    }
    pub(super) fn child(parent: &Directory, name: &str, create: bool) -> Result<Directory> {
        // 父目录句柄不共享 DELETE；整个操作期间攻击者不能替换父目录。
        let _ = parent.handle.as_raw_handle();
        if create { open_directory(&parent.path.join(name), true, true) }
        else {
            let path = parent.path.join(name);
            let handle = open(&path, false, true, true)?;
            Ok(Directory { path, handle, _ancestors: Vec::new() })
        }
    }
    pub(super) fn file(parent: &Directory, name: &str, create: bool) -> Result<File> { open(&parent.path.join(name), create, false, true) }
    pub(super) fn remove(parent: &Directory, name: &str, directory: bool) -> Result<()> {
        if directory { std::fs::remove_dir(parent.path.join(name))?; }
        else { std::fs::remove_file(parent.path.join(name))?; }
        Ok(())
    }
}
