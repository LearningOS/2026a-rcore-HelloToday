//! Implementation of  [`ProcessControlBlock`]

use super::id::RecycleAllocator;
use super::manager::insert_into_pid2process;
use super::TaskControlBlock;
use super::{add_task, SignalFlags};
use super::{pid_alloc, PidHandle};
use crate::fs::{File, Stdin, Stdout};
use crate::mm::{translated_refmut, MemorySet, KERNEL_SPACE};
use crate::sync::{Condvar, Mutex, Semaphore, UPSafeCell};
use crate::trap::{trap_handler, TrapContext};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefMut;

/// Process Control Block
pub struct ProcessControlBlock {
    /// immutable
    pub pid: PidHandle,
    /// mutable
    inner: UPSafeCell<ProcessControlBlockInner>,
}

/// Inner of Process Control Block
pub struct ProcessControlBlockInner {
    /// is zombie?
    pub is_zombie: bool,
    /// memory set(address space)
    pub memory_set: MemorySet,
    /// parent process
    pub parent: Option<Weak<ProcessControlBlock>>,
    /// children process
    pub children: Vec<Arc<ProcessControlBlock>>,
    /// exit code
    pub exit_code: i32,
    /// file descriptor table
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,
    /// signal flags
    pub signals: SignalFlags,
    /// tasks(also known as threads)
    pub tasks: Vec<Option<Arc<TaskControlBlock>>>,
    /// task resource allocator
    pub task_res_allocator: RecycleAllocator,
    /// mutex list
    pub mutex_list: Vec<Option<Arc<dyn Mutex>>>,
    /// semaphore list
    pub semaphore_list: Vec<Option<Arc<Semaphore>>>,
    /// condvar list
    pub condvar_list: Vec<Option<Arc<Condvar>>>,
    ///当前进程是否启用了死锁检测
    pub deadlock_detect_enabled: bool,
    ///各个mutex当前可用的资源数量
    pub mutex_available: Vec<usize>,
    ///各线程已经持有的mutex数量
    pub mutex_allocation: Vec<Vec<usize>>,
    ///各线程当前还在申请的mutex数量
    pub mutex_need: Vec<Vec<usize>>,
    ///各个semaphore当前可用的资源数量
    pub semaphore_available: Vec<usize>,
    ///各线程已经持有semaphore的资源数量
    pub semaphore_allocation: Vec<Vec<usize>>,
    ///各线程当前还在申请的semaphore资源数量
    pub semaphore_need: Vec<Vec<usize>>,
}

impl ProcessControlBlockInner {
    #[allow(unused)]
    /// get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// allocate a new file descriptor
    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }
    /// allocate a new task id
    pub fn alloc_tid(&mut self) -> usize {
        self.task_res_allocator.alloc()
    }
    /// deallocate a task id
    pub fn dealloc_tid(&mut self, tid: usize) {
        self.task_res_allocator.dealloc(tid)
    }
    /// the count of tasks(threads) in this process
    pub fn thread_count(&self) -> usize {
        self.tasks.len()
    }
    /// get a task with tid in this process
    pub fn get_task(&self, tid: usize) -> Arc<TaskControlBlock> {
        self.tasks[tid].as_ref().unwrap().clone()
    }

    ///为线程初始化mutex和semaphore资源记录
    pub fn init_thread_resources(&mut self, tid: usize) {
        let mutex_count = self.mutex_available.len();
        let semaphore_count = self.semaphore_available.len();

        while self.mutex_allocation.len() <= tid {
            self.mutex_allocation.push(vec![0; mutex_count]);
            self.mutex_need.push(vec![0; mutex_count]);
        }
        while self.semaphore_allocation.len() <= tid {
            self.semaphore_allocation
                .push(vec![0; semaphore_count]);
            self.semaphore_need.push(vec![0; semaphore_count]);
        }

        self.mutex_allocation[tid] = vec![0; mutex_count];
        self.mutex_need[tid] = vec![0; mutex_count];
        self.semaphore_allocation[tid] = vec![0; semaphore_count];
        self.semaphore_need[tid] = vec![0; semaphore_count];
    }

    ///为新创建的mutex初始化资源记录
    pub fn init_mutex_resource(&mut self, mutex_id: usize) {
        let required_len = mutex_id + 1;
        if self.mutex_available.len() < required_len {
            self.mutex_available.resize(required_len, 0);
        }
        self.mutex_available[mutex_id] = 1;

        let mutex_count = self.mutex_available.len();
        for allocation in self.mutex_allocation.iter_mut() {
            allocation.resize(mutex_count, 0);
            allocation[mutex_id] = 0;
        }
        for need in self.mutex_need.iter_mut() {
            need.resize(mutex_count, 0);
            need[mutex_id] = 0;
        }
    }

    ///为新创建或重新使用的semaphore初始化资源记录
    pub fn init_semaphore_resource(&mut self, semaphore_id: usize, res_count: usize) {
        let required_len = semaphore_id + 1;
        if self.semaphore_available.len() < required_len {
            self.semaphore_available.resize(required_len, 0);
        }
        self.semaphore_available[semaphore_id] = res_count;

        let semaphore_count = self.semaphore_available.len();
        for allocation in self.semaphore_allocation.iter_mut() {
            allocation.resize(semaphore_count, 0);
            allocation[semaphore_id] = 0;
        }
        for need in self.semaphore_need.iter_mut() {
            need.resize(semaphore_count, 0);
            need[semaphore_id] = 0;
        }
    }

    ///判断给定的资源分配状态是否安全
    fn is_safe(available: &[usize], allocation: &[Vec<usize>], need: &[Vec<usize>]) -> bool {
        let thread_count = allocation.len();
        let resource_count = available.len();
        let mut work = available.to_vec();
        let mut finish = vec![false; thread_count];

        loop {
            let mut found = false;
            for tid in 0..thread_count {
                if finish[tid] {
                    continue;
                }
                let can_finish = (0..resource_count).all(|rid| need[tid][rid] <= work[rid]);
                if can_finish {
                    for rid in 0..resource_count {
                        work[rid] += allocation[tid][rid];
                    }
                    finish[tid] = true;
                    found = true;
                }
            }
            if !found {
                break;
            }
        }

        finish.iter().all(|finished| *finished)
    }

    ///判断当前mutex资源分配状态是否安全
    pub fn mutex_is_safe(&self) -> bool {
        Self::is_safe(
            &self.mutex_available,
            &self.mutex_allocation,
            &self.mutex_need,
        )
    }

    ///判断当前semaphore资源分配状态是否安全
    pub fn semaphore_is_safe(&self) -> bool {
        Self::is_safe(
            &self.semaphore_available,
            &self.semaphore_allocation,
            &self.semaphore_need,
        )
    }
}

impl ProcessControlBlock {
    /// inner_exclusive_access
    pub fn inner_exclusive_access(&self) -> RefMut<'_, ProcessControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// new process from elf file
    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        trace!("kernel: ProcessControlBlock::new");
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        // allocate a pid
        let pid_handle = pid_alloc();
        let process = Arc::new(Self {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        // 0 -> stdin
                        Some(Arc::new(Stdin)),
                        // 1 -> stdout
                        Some(Arc::new(Stdout)),
                        // 2 -> stderr
                        Some(Arc::new(Stdout)),
                    ],
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    deadlock_detect_enabled: false,
                    mutex_available: Vec::new(),
                    mutex_allocation: vec![Vec::new()],
                    mutex_need: vec![Vec::new()],
                    semaphore_available: Vec::new(),
                    semaphore_allocation: vec![Vec::new()],
                    semaphore_need: vec![Vec::new()],
                })
            },
        });
        // create a main thread, we should allocate ustack and trap_cx here
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&process),
            ustack_base,
            true,
        ));
        // prepare trap_cx of main thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        let ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();
        drop(task_inner);
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            ustack_top,
            KERNEL_SPACE.exclusive_access().token(),
            kstack_top,
            trap_handler as usize,
        );
        // add main thread to the process
        let mut process_inner = process.inner_exclusive_access();
        process_inner.tasks.push(Some(Arc::clone(&task)));
        drop(process_inner);
        insert_into_pid2process(process.getpid(), Arc::clone(&process));
        // add main thread to scheduler
        add_task(task);
        process
    }

    /// Only support processes with a single thread.
    pub fn exec(self: &Arc<Self>, elf_data: &[u8], args: Vec<String>) {
        trace!("kernel: exec");
        assert_eq!(self.inner_exclusive_access().thread_count(), 1);
        // memory_set with elf program headers/trampoline/trap context/user stack
        trace!("kernel: exec .. MemorySet::from_elf");
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        let new_token = memory_set.token();
        // substitute memory_set
        trace!("kernel: exec .. substitute memory_set");
        {
            let mut process_inner = self.inner_exclusive_access();
            //用新程序的地址空间替换旧地址空间
            process_inner.memory_set = memory_set;
            //exec 后新程序需要重新决定是否启用死锁检测
            process_inner.deadlock_detect_enabled = false;
            //旧程序创建的同步对象不应继续留给新程序使用
            process_inner.mutex_list.clear();
            process_inner.semaphore_list.clear();
            process_inner.condvar_list.clear();
            //清空旧程序的 mutex 资源账本，并保留主线程 T0 的空行
            process_inner.mutex_available.clear();
            process_inner.mutex_allocation = vec![Vec::new()];
            process_inner.mutex_need = vec![Vec::new()];
            //清空旧程序的 semaphore 资源账本，并保留主线程 T0 的空行
            process_inner.semaphore_available.clear();
            process_inner.semaphore_allocation = vec![Vec::new()];
            process_inner.semaphore_need = vec![Vec::new()];
        }
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        trace!("kernel: exec .. alloc user resource for main thread again");
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;
        task_inner.res.as_mut().unwrap().alloc_user_res();
        task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        // push arguments on user stack
        trace!("kernel: exec .. push arguments on user stack");
        let mut user_sp = task_inner.res.as_mut().unwrap().ustack_top();
        user_sp -= (args.len() + 1) * core::mem::size_of::<usize>();
        let argv_base = user_sp;
        let mut argv: Vec<_> = (0..=args.len())
            .map(|arg| {
                translated_refmut(
                    new_token,
                    (argv_base + arg * core::mem::size_of::<usize>()) as *mut usize,
                )
            })
            .collect();
        *argv[args.len()] = 0;
        for i in 0..args.len() {
            user_sp -= args[i].len() + 1;
            *argv[i] = user_sp;
            let mut p = user_sp;
            for c in args[i].as_bytes() {
                *translated_refmut(new_token, p as *mut u8) = *c;
                p += 1;
            }
            *translated_refmut(new_token, p as *mut u8) = 0;
        }
        // make the user_sp aligned to 8B for k210 platform
        user_sp -= user_sp % core::mem::size_of::<usize>();
        // initialize trap_cx
        trace!("kernel: exec .. initialize trap_cx");
        let mut trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            task.kstack.get_top(),
            trap_handler as usize,
        );
        trap_cx.x[10] = args.len();
        trap_cx.x[11] = argv_base;
        *task_inner.get_trap_cx() = trap_cx;
    }

    /// Only support processes with a single thread.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        trace!("kernel: fork");
        let mut parent = self.inner_exclusive_access();
        assert_eq!(parent.thread_count(), 1);
        // clone parent's memory_set completely including trampoline/ustacks/trap_cxs
        let memory_set = MemorySet::from_existed_user(&parent.memory_set);
        // alloc a pid
        let pid = pid_alloc();
        // copy fd table
        let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
        for fd in parent.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }
        // create child process pcb
        let child = Arc::new(Self {
            pid,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    deadlock_detect_enabled: false,
                    mutex_available: Vec::new(),
                    mutex_allocation: vec![Vec::new()],
                    mutex_need: vec![Vec::new()],
                    semaphore_available: Vec::new(),
                    semaphore_allocation: vec![Vec::new()],
                    semaphore_need: vec![Vec::new()],
                })
            },
        });
        // add child
        parent.children.push(Arc::clone(&child));
        // create main thread of child process
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&child),
            parent
                .get_task(0)
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .ustack_base(),
            // here we do not allocate trap_cx or ustack again
            // but mention that we allocate a new kstack here
            false,
        ));
        // attach task to child process
        let mut child_inner = child.inner_exclusive_access();
        child_inner.tasks.push(Some(Arc::clone(&task)));
        drop(child_inner);
        // modify kstack_top in trap_cx of this thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);
        insert_into_pid2process(child.getpid(), Arc::clone(&child));
        // add this thread to scheduler
        add_task(task);
        child
    }
    /// get pid
    pub fn getpid(&self) -> usize {
        self.pid.0
    }
}
