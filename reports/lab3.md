# lab3

# 功能总结

将上一章节的`sys_get_time` `sys_mmap` `sys_munmap`进行迁移，以适应新的进程结构；实现了使用spawn进行进程创建，其根据目标ELF创建独立的地址空间的子进程；接着编写了set\_priority和stride相关函数，实现调度器为每个进程保存优先级和stride，每次切换任务时，选择stride最小的就绪进程，达到CPU时间与优先级大致成正比的效果。



# 问答题

## 问题1 当8位无符号数值溢出后，实际是否能轮到P1运行？

不会，P2本次执行后，stride为4，，按照简单的大小比较，调度器会认为255\>4，从而会再次选择P2，而不会选择理论上stide更小的P1。



## 问题2 为什么进程优先级 \>= 2 时，STRIDE\_MAX – STRIDE\_MIN \<= BigStride / 2？请解释

最开始的时候，所以的进程的stride都是0，每次调度器选择stride最小的进程，并将它增加BigStride / priority，由于限定所有优先级均不小于 2，所以每个进程的一次增量都不会超过BigStride / 2，因此在当前最小值被更新后，不会一次跨越超过BigStride / 2，其他进程的stride保持不变，那么STRIDE\_MAX – STRIDE\_MIN \<= BigStride / 2恒成立。



## 问题3 怎么样实现让stride支持溢出比较？

可以对两个进程的stride执行允许溢出的减法，再把结果的同一组而紧凑位解释为有符号整数。在满足STRIDE\_MAX – STRIDE\_MIN \<= BigStride / 2前提下，使用wrapping\_sub函数，对两个进程的stride执行允许溢出的减法，如果为负数，则自身进程在环形顺序中更小，否则另一个更小。如果使用BinaryHeap\<Stride\> 的 pop 方法能返回真正最小的 Stride的效果的话，还需要使用反转法，才能让pop取出逻辑上最小的stride。

```C++
use core::cmp::Ordering;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let difference = self.0.wrapping_sub(other.0) as i64;

        if difference < 0 {
            Some(Ordering::Greater)
        } else {
            Some(Ordering::Less)
            
        }
    }
}

impl PartialEq for Stride {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
```



# 荣誉准则说明

1. 在完成本实验及前章功能迁移期间，我曾使用 OpenAI辅助分析编译错误和测试输出，梳理 `spawn`、`Arc/Weak` 父子关系、`set_priority`、Stride 调度、环形整数溢出比较以及前章系统调用迁移等概念。实验代码由我结合实验要求手动编写、运行测试并逐步验证。

2. 我参考了 rCore Tutorial Guide 第五章练习、rCore\-Tutorial\-Book\-v3、项目源码及官方用户态测试，用于确认系统调用接口、进程创建语义、调度公平性标准和预期行为。

3. 除上述已经披露的交流和资料外，其余实验代码与文档由我独立完成。我理解外部帮助可能降低实验难度，并可能影响评分。

4. 我没有复制或变换他人的实验代码，也不会向他人公开自己的实验实现；提交内容无意破坏或妨碍评测及其他计算机系统，并接受课程纪律约束。

