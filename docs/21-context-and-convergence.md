# 21：上下文与收敛——状态为什么会膨胀

[敏感性一课](05-sensitivity.md)解释了为什么同一个块需要暂时保留多份状态；
[全局模型与详细路径](20-global-and-local.md)说明了 Global 与 Native 怎样共同表达一次分析。
本课接着问：这些区别为什么会越积越多？怎样讨论收敛设计，才能同时看清覆盖、精度与费用？

先看两个块跳向同一段代码的情形。下列名字都是字节码块标签，省略号代表与跳转无关的指令：

```text
cA: PUSH retA; PUSH H; JUMP
cB: PUSH retB; PUSH H; JUMP
H:  ... ; JUMP                 ← 使用栈里保留的返回地址
```

`retA`、`retB` 是调用者的继续位置，称为 continuation。这里没有执行 CALL opcode。
若两次进入 H 的栈值立即合并，返回目标可能变成 `{retA, retB}`；先保留 cA、cB 的区别，可以避免交叉返回。
但若 H 内部经过许多分支，所有近期跳转都继续区分状态，保留区别的成本也会扩散到更多块。
于是，“哪里需要区别”和“区别应保留多久”成为两个需要一起回答的问题。

## 1. 先分清：键增加，还是键内的信息增加

**状态键**决定哪些输入进入同一个分区；**payload（载荷）**保存这个分区中的抽象机器信息。
键相同的两个输入仍可能有不同栈值、memory、Store 或关系约束，因此需要覆盖性合并。
Store 是执行中的共享账户状态，包含 storage 等效果；它与每个 frame 私有的栈、memory 分开。

```text
新键 K₂ 到达                 → 新建分区，保留另一份输入
旧键 K₁ 到达更宽的输入       → 更新 K₁ 的 payload，可能重新求值和传播
旧键 K₁ 到达已被覆盖的输入   → 不必增加语义输入，但查找、比较仍有费用
```

所以键数量有界，不等于输入已经稳定，也不等于一次请求便宜。
本课说的健全覆盖，是抽象变换仍包含其声明输入与语义允许的全部具体行为。
固定点要求相关输入、输出与依赖在相应前提下稳定；预算耗尽后队列停止，不构成这种稳定的证明。
策略接口及覆盖性准入的现状见[状态策略](state-policies.md)，这里关注这些选择造成的现象。

### RecentDistinctJumps 既去重，也保留顺序

当前 [`RecentDistinctJumps`](../crates/evm-abstract/src/analysis/engine/policy.rs) 在同一跳转来源再次出现时，
先删除它的旧位置，再放到末尾；超过 `context_depth` 才删除最老的项。
这里的来源是 JUMP／JUMPI 所在块的起始 PC，不是跳转目的地址。下面按“最老 → 最新”排列：

```text
[a, b, c] 再经过 b  → [a, c, b]
[a, b, c] 再经过 a  → [b, c, a]
```

两条结果包含相同的站点集合，却是不同的有序历史。distinct 消除了重复站点，并没有消除排列。
即使程序只有有限个跳转站点，长度有界的历史仍允许大量排列；实际能到达哪些排列由控制流决定。
循环不会只因反复经过同一站点就无限加长列表，但复杂分支仍可能不断产生此前未见的键。

当前 [`FeedbackAcceleration`](../crates/evm-abstract/src/analysis/engine/policy/semantic.rs) 在已确认反馈、更新达到策略条件时请求 widening。
**Widening** 是覆盖旧输入和新输入、并为稳定增长提供加速的抽象操作。
它作用于某个已准入输入；不同 history 先变成不同键时，一个键内的数值 widening 不会自动合并其他键。

### “详细区域”当前选择的是完整来源

[`hybrid/policy.rs`](../crates/evm-abstract/src/analysis/hybrid/policy.rs) 按捕获来源的代码地址、hash 和 Runtime 角色选择详细上下文。
当前选择单位是完整 captured source，不是内部子程序，也不是某几个热点块。
因此同一来源在未知环境的回调内执行时，仍可能保留其跳转历史；换了 owner 或 caller 不会自行取消来源选择。
每个新 frame 的初始历史仍为空，这不表示回调内后来形成的历史都会被清除。

未选中来源在实际准入前清除可选历史，相关准备和拒绝后的恢复见
[`policy/history.rs`](../crates/evm-abstract/src/analysis/hybrid/policy/history.rs)。
来源、owner、static、真实 continuation 和 rollback 检查点仍有各自的表示与兼容要求。
不能由“只选一个来源”推导“整个世界只会有一小片详细状态”。

### 先确认增长发生在哪张表

外层 World 状态保存整机执行位置；相对过程的 **Body** 节点保存过程内部待求值的输入。
**Resume** 负责将 callee 结果投递回订阅它的 caller；subscriber 保存该 caller 的续点与检查点。
这些内部节点与 World 状态分属不同命名空间，同样的数字 ID 不能当作同一个图节点。
一次求值的 receipt（执行凭据）记录所解释的输入版本及实际保留的转换；pending 表示还有待处理义务。
receipt 的 current 资格还要匹配相应输入与来源前提，不能仅由“曾经执行过”推出。

[`过程求解器`](../crates/evm-abstract/src/analysis/transfer/procedure/solver.rs) 在各自过程的键索引里查找 Body：
已有键走输入准入，新键才分配节点；内部新节点也消耗共享的状态分配额度。
procedure 数、subscriber 数、输入版本与 transfer 次数，各自回答不同问题。
只把外层图节点相加，可能解释不了累计分配；把历史模型快照反复相加，也会重复计算保留节点。

同一站点集合的不同顺序可以形成多个完整键；仅看到同 PC 或同一键投影重复，不能证明完整键被重复准入。
压低 `context_depth` 可以减少历史区别，却也可能让输入更宽，增加后继、重新求值与摘要投递。
即使统一设成很小的值，也不保证更快或收敛；减少分配与降低总 work 是需要分别验证的结果。
判断候选合并是否安全，还须检查覆盖与返回目标，不能只比较键的数量。

## 2. Gigahorse：为什么及时忘记历史有时更精确

讨论 Gigahorse 时，要先分清论文代际与当前工具链。

| 代际 | 与本课有关的上下文选择 |
| --- | --- |
| [2019 原 Gigahorse，§V](https://yanniss.github.io/gigahorse-icse19.pdf) | call-site sensitivity，论文默认 1-call-site／1-CFA |
| [2022 Elipmoc，§4.2](https://yanniss.github.io/elipmoc-oopsla22.pdf) | TransactionalContext，组合 public 入口与私有控制历史 |
| [2025 Shrnkr，§3.4／§4／§5](https://yanniss.github.io/shrnkr-full.pdf) | shrinking、选择性 early cloning、不完整全局 pre-analysis；已有 [2024 arXiv 预公开](https://arxiv.org/abs/2409.11157) |

当前同一个 `gigahorse-toolchain` 包含这些发展；默认选择 `TransactionalWithShrinkingContext`，
见[配置说明][gh-default]与[主入口][gh-main]。不能用 2019 年的默认策略概括今天的实现。
以下源码链接固定到同一提交，讨论的是这套实现的规则，而非本项目已采用的能力。

### 上下文不是实际调用栈

其[上下文结构][gh-context]可以读成：

```text
ctx = (public entry selector, bounded private block history)
       相对持久的入口区别       有界的内部块历史
```

public 分量保留识别出的入口 selector（通常来自 calldata 开头四字节），private 分量近似记录内部调用、返回等控制来源。
[Transactional][gh-transactional] 在疑似 call 和疑似 return 上都 push 当前块，普通块不记入这份历史。
它记录的是有选择的控制历史，不是真实的 push-call／pop-return 调用栈。

[Shrinking][gh-shrinking] 在疑似 call 上仍 push 当前块；遇到 return 候选边时，先看该边实际恢复出的目标 `cont`。
若历史中存在符合条件的 call／continuation 匹配，就删除最新历史直到匹配 call，连该 call 一起删除，留下更老的外层历史。
这不是清空全部历史、只 pop 一个元素，或按固定深度一刀切；也不是把任意 JUMP 都当作返回。
不能可靠匹配时，规则继续保留 return 块历史。

### 匹配依据来自栈值流和字节码启发式

[局部规则][gh-local] 从字节码找候选：同块知道跳转目标，且有逃逸的 JUMPDEST 常量留给后续使用，看起来像 call；
同块不能恢复跳转目标的 JUMP／JUMPI，看起来像 return。
“逃逸”指常量没有在本块被消费，仍留在输出栈中；这些名称表达启发式分类，不是函数语义已经被证明。

一个 call 块可能留下多个 continuation。默认 `firstContinuation` 选栈中最深的候选，
且不能有更浅的同值重复；它不表示“最近一次压入”或“马上第一次跳往”的地址。
pre-analysis 还会依据后续控制流中的 `LastContinuation` 修正这个选择，见[两阶段连接][gh-global]。
多个 continuation、复制与复用，使简单地按压栈先后解释这项规则并不可靠。

查找匹配也不能越过更近的歧义：如果较新的 call 也推过同一个 `cont`，却不满足所需的 first-continuation 条件，
不能穿过它去匹配更老的 caller。无法安全剪切时继续记录 return 历史。
具体的阻挡条件与剪切规则见[匹配规则][gh-cut]，不应把它简化成“找到历史上任意相同返回地址就 pop”。

不同 caller／历史中的栈值先按上下文分开传播；[全局值流规则][gh-value-flow]用这些值确定候选合法跳转目标，
然后针对候选边请求上下文更新。历史参与分组和匹配，不独自猜出正确返回地址。
若返回目标已经混在一起，剪短历史本身不会重新创造丢失的值关系。

### 一个深度为 4 的小表

下面只列 private 分量，并改用“最新 → 最老”的顺序，与前面的 RecentDistinctJumps 示例方向相反。
A、B 是两份不同的外层块历史；c1、c2 是依次进入的疑似 call 块，r2、r1 是对应 return 块。
假设两条返回边的目标已由值流恢复，而且分别满足 c2、c1 的匹配条件；public 分量保持相同。

| 经过的块 | Transactional：从 A／B 出发 | Shrinking：从 A／B 出发 |
| --- | --- | --- |
| 初始 | `[A]` ／ `[B]` | `[A]` ／ `[B]` |
| c1 | `[c1,A]` ／ `[c1,B]` | `[c1,A]` ／ `[c1,B]` |
| c2 | `[c2,c1,A]` ／ `[c2,c1,B]` | `[c2,c1,A]` ／ `[c2,c1,B]` |
| r2 | `[r2,c2,c1,A]` ／ `[r2,c2,c1,B]` | `[c1,A]` ／ `[c1,B]` |
| r1 | `[r1,r2,c2,c1]`，A／B 都丢失 | `[A]` ／ `[B]` |

Transactional 的四个内层记录挤掉外层区别，之后到达同一块的输入可能合并。
Shrinking 及时忘掉已经结束的内层历史，反而给外层区别腾出空间。
这说明“保留更多经过的块”不等于“保留更多有用区别”；它不保证每次 cut 都能无损精度。

### pre-analysis 与 early cloning 各做一部分准备

不完整全局 pre-analysis 用关键关系容量等限额控制推导工作，是尽力分析，不要求先完成整个主分析。
它过滤伪 public／private call，检查 selector 或 continuation 的实际用途，
还找出引入 imprecision 的边，让主分析在这些边额外保留上下文。
这里的 imprecision 是本来可分开的栈来源在合流处混合；规则见[预分析][gh-preanalysis]、[合流识别][gh-imprecision]与[阶段连接][gh-global]。

early cloning 则先改变被分析的块表示。多个 PUSH 来源使用同一个共享 continuation 等候选块时，
为相应 PUSH 来源复制候选块，并重写指向副本的地址，使同块的不同用途更容易分开。
它是选择性复制，不是为每条执行路径克隆全部代码；候选选择见[cloning 规则][gh-cloning]。
具体的按 PUSH 来源复制与地址替换见[重写规则][gh-rewrite]。
若一个逃逸地址由 DUP 复制，预处理还可把该 DUP 转成等值 PUSH，显露两个用途，见[DUP 转换][gh-dup]。

### 它还控制了每个上下文要传播多少信息

当前主要 global CFG 阶段使用 Optimized 分析，跨块传播的栈值限制在 jumpdest 相关值与 selector，
不是在每个上下文里传播完整的一般机器载荷。一般栈值与 SSA 的恢复在后续 functions 等阶段继续完成，
见[传播投影][gh-optimized]与[管线顺序][gh-main]。因此上下文数量之外，每份上下文的成本也不同。

Datalog 把结果表示成集合事实：相同事实再次推导出来，不会成为另一条新事实。
Soufflé 的半朴素求值让递归推导至少使用一个新增事实 delta，再与其他已有事实组合，避免每轮重做全部旧组合，
见[官方技术报告 §4，第 16–17 页](https://souffle-lang.github.io/pdf/provenance-tr.pdf#page=16)。
这里说的是求值机制，不是某条 Gigahorse 规则显式维护了 delta。
集合去重和增量传播降低重复推导，却不会消除“上下文 × 块 × 栈位置 × 候选值”的组合增长。
它们也不等于已经自动解决完整 Store、表达式关系与回滚载荷的更新成本。

### 比较时保留任务范围与实现边界

Gigahorse 这条管线主要解决单份 bytecode 内的 CFG／值流恢复。
本项目还分析整个 world、未知实现、CALL／DELEGATECALL／CREATE、回调与 reentry、Store 和 rollback。
同一 bytecode 内的 jump-history 与跨账户真实调用帧是不同轴，不能直接用前者的工作量比较后者的全部任务。

其[合法跳转目标规则][gh-value-flow]依赖恢复出的目标值；未知 jump 没有自动连向所有合法 JUMPDEST 的 fallback，因而可能缺边。
有限 stack 配置、启发式与分析限额也各有边界，应按这项 CFG 任务的实际覆盖理解结果。
默认策略到历史上限时删去最老记录后继续分析，见[默认截断][gh-truncate]；
scalable fallback 实际使用 [FiniteShrinkingContext][gh-fallback]，到深度限额时不再扩展所需上下文，见[有限规则][gh-finite]。
last-resort 使用 [FinitePreciseContext][gh-last]。不能把有限 fallback 解释为健全的覆盖性 join，
也不能从源码中的 `Complete` 名称推导任意 bytecode 的 soundness。

## 3. 方向一：让局部分区有进入、保留与退役的时机

当前实现按整份来源选择详细上下文，再保存近期不同跳转的顺序。
候选设计是：在有证据的内部调用／返回等局部范围内启用、保留或退役分区，让区别服务于具体的控制问题。
这里的“局部”需要分析证据确定范围，不能只取一个人看起来像 helper 的标签区间。

```text
cA / cB → H 内部分开传播 → 各自确认返回 continuation
                           ↓
                 已结束的内层区别能否在这里退役？
```

H 内部可能需要区分返回目标；回到各自 continuation、再到达相容的汇合位置时，H 的某些分支历史可能已经没有用途。
H 返回的数值、共享状态写入或条件关系仍可能影响后继；可以覆盖性放宽这些信息，不能因此删掉可能行为。
这正是需要评估的边界：退役的是一种分组区别，留下的机器信息仍覆盖全部旧输入，不要求精度原样保留。

健全的合并至少要覆盖所有旧输入及其可能后继，并证明栈形状、frame 角色、来源、owner、static、
返回续点与 rollback 检查点相容。新输入到达后，要继续进入覆盖后的分区，相关后继与订阅也须重新有效。
若无法证明调用／返回匹配，可以保留区别，也可以可靠地粗合并并保留全部可能目标；只有覆盖尚未建立时才保留 frontier。
不能凭一个标签删除可能路径。
若边界不兼容，合并必须拒绝，而不是对两个不同形状的栈逐槽求并集。

Gigahorse 提供的是“在有用途的边界及时忘记”的经验，本项目仍需给完整机器状态建立覆盖契约。
现有[策略入口](state-policies.md)可以承接实验，但现有 Trait 名称不代表局部生命周期设计已经确定。

## 4. 方向二：共享闭合未知包络，按需求细化回调

当前 Global 已在有限 owner／source 类中执行未知操作过程，CALL 类动作、创建、返回与回滚共同进入 Body／Resume 固定点。
输入汇合到 callee，输出更新投递给 subscribers；每个 caller 保留自己的续点和 rollback 检查点。
已知控制还按来源限定的块与 MAY 关系传播，见[过程表示](../crates/evm-abstract/src/analysis/process.rs)和[未知世界](18-unknown-world.md)。
Native 也已有 known-call／relative summary 复用，另有[精确前提的调用证书](../crates/evm-abstract/src/analysis/summary.rs)，并非从未复用。
Global 与 Native 当前并不直接互相导入过程表；未来共享更多结果时，仍须完成语义转换与覆盖认证。

候选设计是在这些能力之上保持闭合的 **unknown envelope（未知行为包络）**，
把更多详细成本留给需要回答问题、且已经捕获代码的 callback／局部行为。
闭合包络须覆盖零次、多次、间接和嵌套交互，以及各层成功／失败与 rollback；不能先把回调次数限制为一次。

```text
A → 未知 U → 零次、一次或多次回调 A → U 的 Return / Revert / Failure
         │
         ├─ 合格输入共享保守闭合包络
         └─ 需要区分 A 的某条 callback 时，再增加该部分详细证据
```

`A → U → A → U → …` 会形成循环依赖：相关过程的输入、输出、订阅和恢复关系需要联合稳定，
不能把某次 U 暂时没有新输出当成整条交互链已经闭合。

例如，要回答 U 期间 A 的某个槽可能怎样改变，可以复用未知过程的交互覆盖；
若要解释 A 的特定分支为什么失败，再细化其实际输入与指令路径。
详细路径尚未完成，Global 的可能控制仍应保留；Global 的 MAY 块也不能冒充 Native `executed_pcs` 或 SSA。
细化后的结果能替换哪部分依据，需要独立资格，不能仅因它看起来更详细就替换整个模型。

复用的健全条件首先是**本次调用输入被旧摘要的适用范围覆盖**，包括 calldata、Store 与关系。
还要认证固定快照 snapshot、初始事实修订 fact revision，以及冻结来源／owner 词汇的 source epoch；
owner、逻辑 ADDRESS、CALLER、value、static、读写条件和 rollback 检查点也必须相容。
不同 caller 的继续位置可以分别订阅同一合格 callee，但不能让一个 caller 的检查点变成另一个的回滚依据。
精确调用证书与相对汇合摘要使用的认证方式可以不同，二者都不能只凭相同 codehash 或 key 套用旧结果。

输入增长时，旧输出可能不再覆盖新输入，必须重新建立覆盖，必要时重算，并把变化投递给订阅者。
外部前提退役时，也须使依赖资格失效。
调用和回调可能改变 Store；恢复后使用的旧读取身份与摘要，必须仍适用于此时的有效输入与前提。
同数值重新证明，也不自动恢复旧来源资格；[来源与发布机制](../crates/evm-abstract/src/analysis/cooperation/global.rs)区分增量结果与闭合资格。
具体重算、重新认证或投递范围可以优化，但不能省略相应义务。

这条方向不是把未知 U 简化成“任意改 Store 后返回”，也不是把回调链完整逐层展开作为复用前提。
它要回答的是：哪些闭合覆盖可以共享，哪些 callback 细节值得单独付费，以及新信息怎样使旧依赖重新有效。

## 5. 方向三：减少合并后真正需要重做的载荷工作

即使方向一减少了键，方向二增加了摘要复用，剩余输入仍可能越来越宽。
当前已有集合去重、字节 DAG 共享、表达式／关系容量上限、guarded join、feedback widening，
以及输入变化驱动的求值和摘要更新；不能把“再加一个去重表”当作完整方案。
字节 DAG 是共享不可变子结构的有向无环图；guarded join 则用共同的选择条件保留整机替代之间的配对。

例如两份输入分别满足 `(x=0,y=1)` 和 `(x=1,y=2)`。独立合成 `x∈{0,1}`、`y∈{1,2}`，
会允许 `(0,2)` 等额外组合，丢失 `y=x+1`。用同一选择条件 `s` 表示
`x=ite(s,0,1)`、`y=ite(s,1,2)` 可以保留配对，但持续汇合也会增加表达式和比较成本。
这是表示取舍的概念例子，不表示当前任意摘要都保留该关系。

```text
两份输入原来分开：栈顶分别为 0、1，各走一条后继
             合并：栈顶覆盖 {0,1}
                  ↓
           后继变多，旧后继输入也可能再次增长
```

键少了，单键的 input updates、transfer／replay、join／比较、表达式节点和 Store 遍历仍可能增加。
一次输入更新还可能改变 summary 输出，再唤醒多个 Resume；所以分配下降与总 work 上升可以同时发生。
载荷复制与比较本身也收费，见[载荷计费](../crates/evm-abstract/src/analysis/machine/payload.rs)；
[整机 join](../crates/evm-abstract/src/analysis/machine/join.rs)覆盖的是联合状态，不能只比较几个数值槽位。

候选设计是进一步细化现有变化传播与依赖失效：按实际新增事实、实际消费者使用的投影，安排必要工作。
另一个分支是使有效载荷保持有限表示，并把局部精化集中在真正影响控制或查询的位置。
例如一个 subscriber 只消费某类联合出口，新增长若不改变它消费的投影，能否避免重放其他部分，需要认证该投影的完整依赖。
当前 kernel 已有[依赖通知与前提失效](../crates/analysis-kernel/src/kernel/changes.rs)，候选是在其上缩小不必要更新，而非重新发明通知机制。

健全退化仍要联合覆盖栈、memory、Store、返回数据和关系，不能留下排除旧联合状态的 guard 或关系。
表达式身份与关系不能混用不同执行作用域；同 PC、同标签或相同表达式外观不证明两个运行值相等。
[摘要重放](../crates/evm-abstract/src/analysis/summary/replay.rs)为局部身份重新命名，并保留实际输入根的绑定，正是这项区别的现有例子。
前提变化必须重新认证，不能因为投影的数字没变就沿用失效的证明。

局部查询失败时只能保留保守结果或显式 frontier，不能把未求出矛盾解释为不可达。
是否采用另一种 solver、怎样选择有限表示与精化时机，仍是候选设计；验收对象是覆盖和实际更新成本。

## 6. 方向四：把轻量统计从丰富导出中独立出来

在 Global 闭合且来源资格有效时，当前 [`minimum`](../crates/evm-abstract-protocol/src/minimum.rs) 已能提供首份可读的闭合模型，
保留原始输入、来源资格、关系和联合数值出口，不要求先导出完整图或 SSA，见[渐进结果](progressive-results.md)。
它回答“已有哪份可用语义模型”，不能替代 Native 的状态 census（按明确定义清点状态）。
当前已有累计计数、过程记录、[`PhaseTimings`](../crates/evm-abstract/src/analysis/progress.rs) 的阶段耗时，
以及 [`HybridWork`](../crates/evm-abstract/src/analysis/hybrid.rs) 的 Global／准备／Native 等费用划分。
缺少的是费用有界、可独立取得的 Native 状态清点与热点分布，不应靠完整 rich 导出来拼凑。

例如外层 World 很小，相对 Body 却很大时，用户首先需要知道增长落在哪个命名空间和键维度。
如果为此先复制每个 frame 的完整栈、Store 和表达式图，观察本身就可能放大内存与工作问题。

| 统计对象 | 应回答的问题 |
| --- | --- |
| 累计分配 | 到目前为止消耗了多少分配额度，包含此前轮次 |
| 当前保留节点 | 这份模型实际保留多少不同节点，避免重复累计历史快照 |
| outer World／regional Body | 增长在整机图，还是相对过程内部；各自带命名空间 |
| procedure／PC 热点 | 哪个过程、哪个来源限定的块／PC 聚集状态与更新，避免只看全图总量 |
| 完整键与维度投影 | 来源、owner、static、栈形状、history 等区别怎样贡献分区 |
| procedure／subscriber | 有多少过程与等待续点，不把它们自动算作状态分配 |
| 当前／pending／历史 receipt | 哪些输入已被当前求值覆盖，哪些尚待执行或仅有旧观察 |
| summary 输入／输出更新 | 哪个边界持续增长，向多少依赖投递；revision 不是更新费用 |
| transfer／work／队列 | 做了多少求值、累计逻辑费用及待办，分别记录口径 |

候选可以使用少量稳定分桶、有界采样或增量计数；不要求每帧全量 dump。
公共 frame 投影可能省略完整键字段，投影重复也不能证明完整键重复准入。
计数应标明所属阶段、表命名空间、观察版本与是否完成，并区分累计值和瞬时值。
receipt 的 current、input revision 和严格输入更新次数也不能互相代用。

统计的健全条件包括观察语义：采样不能冒充精确总量，历史记录不能冒充最终当前输入，
取消后的 partial 计数不能冒充完成证据。统计自身的扫描、内存、复制和发布也计入预算。
轻量入口必须能在不展开完整 rich／图／SSA 的条件下工作，才能用于判断下一步应优化哪类费用。

Gigahorse 自带的 [Analytics][gh-analytics] 可以报告每块 context 数与缺失跳转目标等问题；
Soufflé 另需显式开启 [profiler](https://souffle-lang.github.io/profiler)，才能按 relation、rule、iteration 查看 tuple 数、耗时与 copy 开销。
这类观测帮助定位费用和覆盖缺口，统计本身不建立 closure，也不能将未完成分析认证为完整结果。

## 7. 怎样判断四个方向是否奏效

四个方向分别处理分区数量、未知交互覆盖的复用、单份载荷的更新费用，以及可观察性；它们可以相互影响。
验收需要同一组对照同时回答以下问题，不能只报告状态总数下降。

| 维度 | 需要保留的证据 |
| --- | --- |
| 覆盖与资格 | 原始输入被覆盖，来源与前提当前，闭合义务已完成；缺口仍有类型化 frontier |
| 实际成本 | CPU、峰值内存、work、transfer、输入／输出更新及 replay／join／比较成本，注明测量边界 |
| 控制精度 | 伪边是否增加，返回点能否区分，callback／reentry、失败与 rollback 是否仍被覆盖 |
| 统计与复现 | 观察自身费用有界，输入、策略、版本和预算一致，取消与未完成样本单列 |

保持原始 world、caller、value、calldata 和未知环境／reentry 范围；限制行为后更快，回答的是另一个问题。
work 是版本化的逻辑费用，不等于 CPU 时间；Global 闭合也不表示 Native 全部输入已经求值。
有限测试可以发现漏掉的返回、回调、创建或回滚行为，不能替代一般覆盖论证。
真正的收敛说明还需要键范围、载荷增长、加速策略和依赖资格一起成立，预算只是可执行边界。

设计背景与未决分支保留在 [#152](https://github.com/ethever/evm-abstract-lab/issues/152)、
[#144](https://github.com/ethever/evm-abstract-lab/issues/144) 和 [#124](https://github.com/ethever/evm-abstract-lab/issues/124)。
后续实现可以选择不同 Trait、solver 或接口；这里需要持续满足的是上述覆盖、更新与交付义务。

[gh-default]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/Advanced.md#L8-L9
[gh-main]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/main.dl#L12-L23
[gh-context]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/context-sensitivity/abstract_context.dl#L149-L158
[gh-transactional]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/context-sensitivity/transactional_context.dl#L23-L31
[gh-shrinking]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/context-sensitivity/transactional_shrinking_context.dl#L38-L125
[gh-local]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/local_components.dl#L640-L729
[gh-global]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/global.dl#L8-L30
[gh-cut]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/context-sensitivity/transactional_shrinking_context.dl#L38-L125
[gh-value-flow]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/global_components.dl#L131-L208
[gh-preanalysis]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/global_components.dl#L354-L392
[gh-imprecision]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/global_components.dl#L288-L316
[gh-cloning]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/statement_insertor.dl#L527-L588
[gh-rewrite]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/statement_insertor.dl#L356-L452
[gh-dup]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/local.dl#L49-L60
[gh-optimized]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/global_components.dl#L322-L347
[gh-truncate]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/context-sensitivity/abstract_context.dl#L244-L255
[gh-fallback]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/fallback_scalable.dl#L2
[gh-finite]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/context-sensitivity/finite_shrinking_context.dl#L81-L86
[gh-last]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/last_resort.dl#L7
[gh-analytics]: https://github.com/nevillegrech/gigahorse-toolchain/blob/1b06cef82f0a29790e87f576eba6d6d793170d70/logic/decompiler_analytics.dl#L121-L253
