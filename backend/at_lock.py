"""串口访问串行锁

telemetry（4G 遥测）与 location（定位采集）都会读 /dev/ttyUSB2。
若两个 cat 读者并发读同一串口，内核通常只把数据交给其中一个，
另一边的响应会错位/丢失（即之前"只看到 CGATT、其余全丢"的根因）。
用 fcntl 建议锁把**所有** AT 访问串行化，谁先抢到谁读，读完再放。

⚠️ 可重入：同一进程内允许嵌套 with at_lock()（例如 collect_cell 外层加锁，
   其内部调用的 at_cmd 又加一次锁）。若不可重入，两次 open 得到两个 fd，
   flock(LOCK_EX) 在第二个 fd 上会死锁等待第一个 fd——正是之前的 bug。
   这里用进程内 RLock + 引用计数保证 flock 只在最外层真正获取/释放一次。
"""
import os
import subprocess
import threading

try:
    import fcntl
    _HAVE_FCNTL = True
except Exception:
    # 非 Linux（如开发机 Windows）无 fcntl，退化为空锁，仅保证模块可导入/测试。
    _HAVE_FCNTL = False

AT_LOCK_PATH = os.getenv("AT_LOCK_FILE", "/var/run/backpack_at.lock")

# 进程内可重入锁 + 引用深度；flock fd 在第一次进入时打开，最后退出时关闭。
_local_rlock = threading.RLock()
_fd = None
_depth = 0


def _acquire():
    global _fd, _depth
    _local_rlock.acquire()
    _depth += 1
    if _HAVE_FCNTL and _fd is None:
        # 只在外层（depth==1 且尚未持有 fd）真正拿跨进程 flock。
        _fd = open(AT_LOCK_PATH, "w")
        fcntl.flock(_fd, fcntl.LOCK_EX)


def _release():
    global _fd, _depth
    _depth -= 1
    if _depth <= 0:
        if _HAVE_FCNTL and _fd is not None:
            try:
                fcntl.flock(_fd, fcntl.LOCK_UN)
            finally:
                _fd.close()
                _fd = None
        _depth = 0
    _local_rlock.release()


class _ATLock:
    def __enter__(self):
        _acquire()
        return self

    def __exit__(self, *exc):
        _release()


def at_lock():
    """用法：with at_lock(): 读取/写入 AT 串口 ..."""
    return _ATLock()


def clear_ghost_cat():
    """清掉可能残留的后台 `cat /dev/ttyUSB2` 孤儿进程。

    这类孤儿 cat 会抢占串口、把模组响应截走（终端能看到 +CEREG/+CSQ，
    但我们用 cat& 的读取方式却拿不到），是"命令发了但读回空、信号失败+
    注册异常"的典型根因。仅针对 cat /dev/ttyUSB2，best-effort，失败不影响主流程。
    """
    try:
        subprocess.run(["pkill", "-f", "cat /dev/ttyUSB2"],
                       check=False, timeout=3,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass
