use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::task::{block_current_and_run_next, current_process, current_task, wakeup_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;
/// sleep syscall
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
/*     let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id as isize
    } else {
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() as isize - 1
    } */
   let mut process_inner = process.inner_exclusive_access();

   let mutex_id = if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id,_)| id)
    {
        //复用以前空出来的 mutex 槽位
        process_inner.mutex_list[id] = mutex;
        id
    }
    else
    {
        //没有空槽位，就在列表末尾增加一把 mutex
        process_inner.mutex_list.push(mutex);
        process_inner.mutex_list.len() - 1
    };

    //mutex_id 同时也是 Available/Allocation/Need 中的资源列号
    process_inner.init_mutex_resource(mutex_id);
    mutex_id as isize
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    //取得当前线程编号：他对应Allocation/Need 的行号
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    //取得当前线程所属进程；mutex 资源账本属于该进程
    let process = current_process();
    //记录本次调用是否真的向 Need 矩阵登记过申请
    let mut need_registered = false;
    {
        let mut process_inner = process.inner_exclusive_access();
        if process_inner.deadlock_detect_enabled {
            //先记录“当前线程想再获得一份 mutex_id”
            process_inner.mutex_need[tid][mutex_id] += 1;
            need_registered = true;

            //如果本次申请会使 mutex 资源状态不安全，撤销临时 Need
            //并拒绝本次申请
            if !process_inner.mutex_is_safe() {
                    process_inner.mutex_need[tid][mutex_id] -= 1;
                    return -0xdead;
            }
        }
        //账本检查完成后，再取得真正的 mutex 对象
        let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());

        //因为 mutex.lock() 可能阻塞并切换到其他线程
        drop(process_inner);
        mutex.lock();
    }
    //mutex.lock() 返回时，当前线程已经真正持有 mutex_id
    let mut process_inner = process.inner_exclusive_access();

    //如果进入 lock 前登记过 Need，现在这次申请已经得到满足
    if need_registered {
        process_inner.mutex_need[tid][mutex_id] -= 1;
    }

    //记录当前线程正式持有了一份 mutex_id
    process_inner.mutex_allocation[tid][mutex_id] += 1;

    //mutex 总共只有一份资源；被当前线程持有后，不再可用
    process_inner.mutex_available[mutex_id] -= 1;

    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );

    //当前线程编号决定应该归还 Allocation 的哪一行
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let process = current_process();

    //先从 PCB 中克隆真正的 mutex 对象
    let mutex = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap())
    };

    mutex.unlock();

    //更新死锁检测账本，表示当前线程不再持有该 mutex
    let mut process_inner = process.inner_exclusive_access();

    //当前线程归还一份 mutex_id
    process_inner.mutex_allocation[tid][mutex_id] -= 1;

    //mutex 总共只有一份资源；解锁后重新变为可用
    process_inner.mutex_available[mutex_id] += 1;


    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        process_inner.semaphore_list.len() - 1
    };
    //id 是资源列号，res_count 是这类资源的初始可用数量
    process_inner.init_semaphore_resource(id, res_count);
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    //tid 决定这次归还属于 Allocation 的哪一行
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;

    let process = current_process();

    //先克隆真实 Semaphore 对象，随后释放 PCB 内部借用
    let sem = {
        let process_inner = process.inner_exclusive_access();
        Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap())
    };
    // 返回 Some(task) 表示这个许可会交给一个等待线程。
    // 此处尚未唤醒该线程。
    let waiting_task = sem.up();

    let mut process_inner = process.inner_exclusive_access();
    //当前线程不再持有这一份 semaphore 资源
    process_inner.semaphore_allocation[tid][sem_id] =
        process_inner.semaphore_allocation[tid][sem_id].saturating_sub(1);

    //账本中该类资源重新增加一份可用资源
    process_inner.semaphore_available[sem_id] += 1;

    if let Some(task) = waiting_task {
        let waiting_tid = task
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid;
        process_inner.semaphore_available[sem_id] -= 1;

        process_inner.semaphore_allocation[waiting_tid][sem_id] += 1;

        process_inner.semaphore_need[waiting_tid][sem_id] = 0;

        drop(process_inner);

        wakeup_task(task);
    }



/*     let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    drop(process_inner);
    sem.up(); */
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    //tid 对应 allocation 和 need 的行号
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    //semaphore 的资源账本属于当前进程
    let process = current_process();
    //记录这一次 down 是否向 Need 矩阵登记过申请
    let mut need_registered = false;

    {
        let mut process_inner = process.inner_exclusive_access();
        if process_inner.deadlock_detect_enabled {
            //当前线程申请一份编号为 sem_id 的 semaphore 资源
            process_inner.semaphore_need[tid][sem_id] += 1;
            need_registered = true;

            // 如果允许这次申请会让 semaphore 资源状态不安全
            // 撤销临时 Need，并拒绝本次 down
            if !process_inner.semaphore_is_safe() {
                process_inner.semaphore_need[tid][sem_id] -= 1;
                return -0xdead;
            }

        }
        //先克隆真实 semaphore 对象，之后才能释放 PCB 的内部借用
        let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
        //sem.down() 可能阻塞当前线程，因此不能带着 PCB 借用进入它
        drop(process_inner);
        //如果资源暂时不足但系统安全，线程会在这里阻塞等待
        let was_blocked = sem.down();

        //如果曾经阻塞，许可、Allocation和Need已经由
        //sys_semaphore_up在唤醒本线程前处理完毕
        if was_blocked {
            return 0;
        }
    }
     // sem.down() 返回时，当前线程已经真正获得了一份 sem_id 资源
     let mut process_inner = process.inner_exclusive_access();
     //如果之前记录了“正在申请”，现在申请已经成功满足
     if need_registered {
        process_inner.semaphore_need[tid][sem_id] -= 1;
     }

     // 当前线程正式持有一份 sem_id 资源
     process_inner.semaphore_allocation[tid][sem_id] += 1;

    // 有一份资源被当前线程持有，当前可用数量减少一
    process_inner.semaphore_available[sem_id] -= 1;
/*     let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    drop(process_inner);
    sem.down(); */
    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}
/// enable deadlock detection syscall
///
/// YOUR JOB: Implement deadlock detection, but might not all in this syscall
///启用或关闭当前进程的死锁检测功能
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    //用户接口只允许 0（关闭）或 1（开启）
    if enabled > 1 {
        return -1;
    }
    //当前系统调用只能修改发起调用的当前进程
    let process = current_process();

    //获取当前进程 pcb 内部的可变访问权限
    let mut process_inner = process.inner_exclusive_access();

    //将用户传入的 0/1 转换成内核保存的 bool
    process_inner.deadlock_detect_enabled = enabled == 1;

    0

}
