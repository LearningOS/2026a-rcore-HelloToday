//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;

///按照环形证书顺序判断left是否位于right之前
fn stride_less(left: usize, right: usize) -> bool {
    (left.wrapping_sub(right) as isize) < 0
}


///A array of `TaskControlBlock` that is thread-saf
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A simple FIFO scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.ready_queue.is_empty(){
            return None;
        }

        let mut min_index = 0;

        for index in 1..self.ready_queue.len() {
            let current_stride = self.ready_queue[index].get_stride();
            let min_stride = self.ready_queue[min_index].get_stride();

            if stride_less(current_stride, min_stride) {
                min_index = index;
            }
        }

        let task = self.ready_queue.remove(min_index).unwrap();
        task.increase_stride();
        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
