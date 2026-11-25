//! 单实例：同一个数据目录只允许一个柜面终端进程。
//!
//! 为什么必须有：两个进程共享同一份 `tx.db` 与菜单包目录，会互相覆盖待重放队列、
//! 幂等索引与审计链——账对不上，而且事后查不出是谁写的。
//! 锁屏也挡不住这个：两个窗口各有一块屏，各以为是唯一真相。
//!
//! 实现只用标准库：给 `<appdata>/.instance.lock` 加**建议性文件锁**（`File::try_lock`）。
//! 为什么不用「PID 文件 + 存活判断」：PID 会复用、崩溃会留脏文件、各平台存活判断
//! 写法不同。文件锁由操作系统在进程退出（含崩溃）时释放，语义正好是我们要的。
//!
//! 锁文件本身**不删除**：删除会在"别人刚拿到锁"的瞬间制造两个进程各持一把锁的窗口。

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 实例锁。持有到进程结束；`Drop` 时显式解锁（进程崩溃时由操作系统释放）。
#[derive(Debug)]
pub struct InstanceLock {
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 尝试取得实例锁。已有实例持有时返回**人能看懂**的错误（给错误页用）。
pub fn acquire(appdata: &Path) -> Result<InstanceLock, String> {
    let path = appdata.join(".instance.lock");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("无法打开实例锁文件 {}：{e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(format!(
                "已有另一个柜面终端实例在运行（数据目录 {}）。请先关闭它，再启动本窗口。",
                appdata.display()
            ))
        }
        Err(TryLockError::Error(e)) => {
            return Err(format!("实例锁不可用（{}）：{e}", path.display()))
        }
    }
    // 记下 PID 只为运维排查；判定不依赖这个内容。
    let _ = file.set_len(0);
    let _ = writeln!(file, "pid={}", std::process::id());
    Ok(InstanceLock { file, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_is_refused_until_the_first_drops() {
        let dir = tempfile::tempdir().expect("临时目录");
        let first = acquire(dir.path()).expect("首个实例应拿到锁");

        let err = acquire(dir.path()).expect_err("第二个实例必须被拒");
        assert!(
            err.contains("另一个柜面终端实例"),
            "错误要让人知道去关掉谁：{err}"
        );

        drop(first);
        let again = acquire(dir.path()).expect("锁释放后应能再次取得");
        assert!(again.path().ends_with(".instance.lock"));
    }

    #[test]
    fn lock_file_records_pid_for_ops() {
        let dir = tempfile::tempdir().expect("临时目录");
        let lock = acquire(dir.path()).expect("应拿到锁");
        let content = std::fs::read_to_string(lock.path()).expect("锁文件可读");
        assert!(
            content.contains(&format!("pid={}", std::process::id())),
            "锁文件应写当前 PID 便于排查：{content}"
        );
    }
}
