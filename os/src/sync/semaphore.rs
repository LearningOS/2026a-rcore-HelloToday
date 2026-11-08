//! Semaphore

use crate::sync::UPSafeCell;
use crate::task::{block_current_and_run_next, current_task, TaskControlBlock};
use alloc::{collections::VecDeque, sync::Arc};

/// semaphore structure
pub struct Semaphore {
    /// semaphore inner
    pub inner: UPSafeCell<SemaphoreInner>,
}

pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Semaphore {
    /// Create a new semaphore
    pub fn new(res_count: usize) -> Self {
        trace!("kernel: Semaphore::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// up operation of semaphore
    /// 归还一个许可；若有等待者，返回应接收该许可的线程
    pub fn up(&self) ->Option<Arc<TaskControlBlock>>{
        trace!("kernel: Semaphore::up");
        let mut inner = self.inner.exclusive_access();
        // 信号量增加一个许可
        inner.count += 1;
        // count <= 0 说明此前有等待者
        // 这个新增许可不会变成空闲许可，而是交给队首等待线程
        if inner.count <= 0 {
            inner.wait_queue.pop_front()
        }
        else {
            None
        }
    }

    /// down operation of semaphore
    pub fn down(&self) -> bool{
        trace!("kernel: Semaphore::down");
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        if inner.count < 0 {
            inner.wait_queue.push_back(current_task().unwrap());
            drop(inner);
            block_current_and_run_next();
            true
        }
        else {
            false
        }
    }
}
