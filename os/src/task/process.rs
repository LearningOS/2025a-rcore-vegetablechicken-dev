//! Implementation of  [`ProcessControlBlock`]

use super::id::RecycleAllocator;
use super::manager::{insert_into_pid2process, wakeup_task};
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
use core::iter;

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
    /// Is enabled deadlock test
    pub enabled_deadlock_test: bool,
    /// Mutex available, meanwhile work
    /// true means the mutex is available, size: mutex_num
    pub mutex_available: Vec<bool>,
    /// mutex allocation, size: task_num * mutex_num
    pub mutex_allocation: Vec<Vec<bool>>,
    /// mutex needed, size: task_num * mutex_num
    pub mutex_needed: Vec<Vec<bool>>,
    /// max semaphore: size: semaphore_num
    pub semaphore_available: Vec<usize>,
    /// semaphore can be allocated, size: semaphore_num
    pub semaphore_work: Vec<usize>,
    /// semaphore already allocated, size: task_num * semaphore_num
    pub semaphore_allocation: Vec<Vec<usize>>,
    /// semaphore needed, size: task_num * semaphore_num
    pub semaphore_needed: Vec<Vec<usize>>,
    /// is task finished, size: task_num
    pub finished: Vec<bool>,
    /// waiting list of task
    pub waiting_list: Vec<Arc<TaskControlBlock>>,
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
    /// get enabled_deadlock_test
    pub fn get_enabled_deadlock_test(&self) -> bool {
        self.enabled_deadlock_test
    }
    /// set enabled_deadlock_test
    pub fn set_enabled_deadlock_test(&mut self, enabled: bool) {
        self.enabled_deadlock_test = enabled;
    }
    /// Init mutex and semaphore info for a new task
    pub fn init_sync_info(&mut self) {
        self.mutex_allocation.push(
            iter::repeat(false).take(self.mutex_list.len()).collect::<Vec<bool>>()
        );
        self.mutex_needed.push(
            iter::repeat(false).take(self.mutex_list.len()).collect::<Vec<bool>>()
        );
        self.semaphore_allocation.push(
            iter::repeat(0).take(self.semaphore_list.len()).collect::<Vec<usize>>()
        );
        self.semaphore_needed.push(
            iter::repeat(0).take(self.semaphore_list.len()).collect::<Vec<usize>>()
        );
        self.finished.push(false);
    }
    /// Update mutex info while creating mutex
    pub fn update_mutex_info_while_creating(&mut self) {
        self.mutex_available.push(true);
        self.mutex_allocation.iter_mut().for_each(|v| v.push(false));
        self.mutex_needed.iter_mut().for_each(|v| v.push(false));
    }
    /// Update semaphore info while creating
    pub fn update_semaphore_info_while_creating(&mut self, count: usize) {
        self.semaphore_available.push(count);
        self.semaphore_work.push(count);
        self.semaphore_allocation.iter_mut().for_each(|v| v.push(0));
        self.semaphore_needed.iter_mut().for_each(|v| v.push(0));
    }
    /// Mutex deadlock test
    /// true means no deadlock
    pub fn no_mutex_deadlock(&self, tid: usize) -> bool {
        let mut allocated: Vec<bool> = iter::repeat(false)
            .take(self.mutex_available.len())
            .collect();
        let mut waiting: Vec<usize> = self.waiting_list.iter()
            .map(|t| t.get_tid())
            .collect();
        waiting.push(tid);
        for val in waiting.iter() {
            for (i, e) in self.mutex_allocation[*val].iter().enumerate() {
                if *e {
                    allocated[i] = true;
                }
            }
        }
        loop {
            let mut need_to_remove = None;
            for (i, val) in waiting.iter().enumerate() {
                let mut can_remove = true;
                for (j, e) in self.mutex_needed[*val].iter().enumerate() {
                    if *e && allocated[j] {
                        can_remove = false;
                        break;
                    }
                }
                if can_remove {
                    need_to_remove = Some(i);
                    break;
                }
            }
            if let Some(i) = need_to_remove {
                for (j, e) in self.mutex_allocation[waiting[i]].iter().enumerate() {
                    if *e {
                        allocated[j] = false;
                    }
                }
                waiting.remove(i);
            } else {
                return false;
            }
            if waiting.is_empty() {
                return true;
            }
        }
    }
    /// find a task to wake up
    pub fn wakeup_task_in_waiting_list(&mut self) {
        let mut allocated: Vec<bool> = iter::repeat(false)
            .take(self.mutex_available.len())
            .collect();
        let waiting: Vec<usize> = self.waiting_list.iter()
            .map(|t| t.get_tid())
            .collect();
        for val in waiting.iter() {
            for (i, e) in self.mutex_allocation[*val].iter().enumerate() {
                if *e {
                    allocated[i] = true;
                }
            }
        }
        let mut need_to_remove = None;
        for (i, val) in waiting.iter().enumerate() {
            let mut can_remove = true;
            for (j, e) in self.mutex_needed[*val].iter().enumerate() {
                if *e && allocated[j] {
                    can_remove = false;
                    break;
                }
            }
            if can_remove {
                need_to_remove = Some(i);
                break;
            }
        }
        if let Some(i) = need_to_remove {
            let tcb = self.waiting_list.remove(i);
            wakeup_task(tcb);
        }
    }
    /// Semaphore deadlock test
    /// true means no deadlock
    pub fn no_semaphore_deadlock(&self, tid: usize) -> bool {
        let mut available = self.semaphore_available.clone();
        let mut waiting: Vec<usize> = self.waiting_list.iter()
            .map(|t| t.get_tid()).collect();
        waiting.push(tid);
        for val in waiting.iter() {
            for (i, e) in self.semaphore_allocation[*val].iter().enumerate() {
                available[i] -= *e;
            }
        }
        loop {
            let mut need_to_remove = None;
            for (i, val) in waiting.iter().enumerate() {
                let mut can_remove = true;
                for (j, e) in self.semaphore_needed[*val].iter().enumerate() {
                    if *e > available[j] {
                        can_remove = false;
                        break;
                    }
                }
                if can_remove {
                    need_to_remove = Some(i);
                    break;
                }
            }
            if let Some(i) = need_to_remove {
                for (j, e) in self.semaphore_allocation[waiting[i]].iter().enumerate() {
                    available[j] += *e;
                }
                waiting.remove(i);
            } else {
                return false;
            }
            if waiting.is_empty() {
                return true;
            }
        }
    }
    /// find a task to wake up
    pub fn wakeup_task_in_waiting_list_semaphore(&mut self) {
        let mut available = self.semaphore_available.clone();
        let waiting: Vec<usize> = self.waiting_list.iter()
            .map(|t| t.get_tid()).collect();
        for val in waiting.iter() {
            for (i, e) in self.semaphore_allocation[*val].iter().enumerate() {
                available[i] -= *e;
            }
        }
        let mut need_to_remove = None;
        for (i, val) in waiting.iter().enumerate() {
            let mut can_remove = true;
            for (j, e) in self.semaphore_needed[*val].iter().enumerate() {
                if *e > available[j] {
                    can_remove = false;
                    break;
                }
            }
            if can_remove {
                need_to_remove = Some(i);
                break;
            }
        }
        if let Some(i) = need_to_remove {
            let tcb = self.waiting_list.remove(i);
            wakeup_task(tcb);
        }
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
                    enabled_deadlock_test: false,
                    mutex_available: Vec::new(),
                    mutex_allocation: Vec::new(),
                    mutex_needed: Vec::new(),
                    semaphore_available: Vec::new(),
                    semaphore_work: Vec::new(),
                    semaphore_allocation: Vec::new(),
                    semaphore_needed: Vec::new(),
                    finished: Vec::new(),
                    waiting_list: Vec::new(),
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
        process_inner.init_sync_info();
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
        self.inner_exclusive_access().memory_set = memory_set;
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
                    enabled_deadlock_test: false,
                    mutex_available: Vec::new(),
                    mutex_allocation: Vec::new(),
                    mutex_needed: Vec::new(),
                    semaphore_work: Vec::new(),
                    semaphore_available: Vec::new(),
                    semaphore_allocation: Vec::new(),
                    semaphore_needed: Vec::new(),
                    finished: Vec::new(),
                    waiting_list: Vec::new(),
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
        child_inner.init_sync_info();
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
