# Nia 编译器架构深度审查：与 Rust、Zig 的实现对照

> 审查日期：2026-07-14
>
> 性质：只读架构审查，不包含语言 feature 横向比较，也不包含本轮代码修改方案的实现
>
> 结论强度标记：**确认**表示可直接由当前源码或实测得到；**推断**表示有明确结构证据、但仍需专项 profiling 验证比例；**建议**表示目标架构判断

## 1. 范围、版本与方法

本报告比较以下本地源码：

- Nia：`/root/project/nia`，commit `b0bcd25f`（2026-07-13）。工作树中已有前序维护改动，本轮不评价这些改动的提交状态，也没有回退它们。
- Rust：`/root/download/rust`，commit `c397dae808f`（2026-07-02）。重点阅读 `rustc_interface`、`rustc_middle`、`rustc_query_impl`、`rustc_monomorphize`、`rustc_codegen_ssa`。
- Zig：`/root/download/zig`，commit `efd6f190fd`（2026-07-02）。重点阅读 `Compilation.zig`、`Zcu.zig`、`Zcu/PerThread.zig`、`InternPool.zig`、`AstGen.zig`、`Sema.zig`、`Air.zig`、codegen 与 link 实现。
- Kern：`/root/project/kern`，commit `1c6fcefb`（2026-06-17）。仅用于解释 Nia 与其前身的性能差异，不把 Kern 当作应当恢复的架构。

审查维度包括：编译生命周期和状态所有权、查询与增量、类型/符号/ID、模块装载、语义分析、IR 分层、monomorphization 与 reachability、codegen/link、并行调度、内存生命周期、诊断与错误边界、crate/API/测试组织，以及冷编译实测。

不比较 Rust/Zig/Nia 的语言 feature 数量。Rust 和 Zig 的成熟度、兼容包袱、目标平台数量明显高于 Nia，因此本报告关注的是它们如何管理复杂度，而不是要求 Nia复制其全部复杂度。

## 2. 执行摘要

Nia 当前的主要瓶颈不是某个慢 pass，也不是“查询数量太多”这一句能概括的。更准确的判断是：**Nia 已经支付了细粒度查询、跨 crate 分层和并发安全的大部分复杂度成本，却还没有建立能让这些成本产生收益的统一编译器内核。**

四个问题互相强化：

1. **会话级语义身份不统一。** 类型由 module-local `TyInterner` 拥有；阶段间通过 clone、snapshot 和 `import_type_into` 搬运。类型 ID 不是整个 compilation session 的统一身份。
2. **查询存储契约鼓励复制。** `QueryKey::Value: Clone` 是通用约束，普通 cache hit 深拷贝值；大产品又常由完整 module/program aggregate 承载。细粒度查询因此没有自动带来细粒度数据流。
3. **依赖图和 fixed point 分裂。** loader query DB、compiler query DB、driver 的 provider discovery 循环、reachability 自己的 fixed point 分别维护“什么依赖什么”。统一增量正确性只能靠多层同步约定维持。
4. **真实工作单元没有进入统一调度器。** `query_many` 临时创建 OS 线程，而 backend lowering、LLVM module codegen 等关键重任务仍主要串行；没有持久 worker pool、jobserver、任务权重、内存预算和 codegen queue。

这四点共同解释了多个表象：

- 冷 `check` 已有 7 秒级耗时和约 490 MiB RSS；
- 70,000 级 query slot/dependency 访问，却只有接近单核的 CPU 利用率；
- interner snapshot/seed 同步遗漏会变成回归；
- provider、type alias、resolver 等 API 容易出现新旧双轨或大量 callback/context glue；
- 64 个 crate 并没有消除巨型实现文件，反而增加依赖扇出和跨边界 DTO；
- 测试需要额外的全局 permit 和并发限制，实际是在外部补偿单次编译的高 RSS 与内部调度缺失。

Rust 的核心优势不是“查询更多”，而是 `GlobalCtxt/TyCtxt`、arena、统一 interner、稳定 dep-node、生成式 query plumbing 和 codegen unit 构成一个经过性能设计的 compiler kernel。Zig 的核心优势不是“单体文件”，而是 `Compilation/Zcu/InternPool/PerThread` 对状态、紧凑 ID、增量依赖和任务所有权有清楚定义。

Nia 不应回到 Kern 的粗粒度全量流水线，也不应继续在当前基础上增加 query、Arc、小 crate 或环境变量。正确方向是：**先重建会话级身份和产品所有权，再重建查询存储与统一调度，最后才做持久增量与 codegen 并行。**

## 3. 结论分级

| 优先级 | 结论 | 当前后果 | 目标 |
|---|---|---|---|
| P0 | 类型、符号、常量缺少统一 session identity | 跨 interner import、snapshot 同步、深递归复制、回归风险 | 一个 compilation-owned semantic context 与全局稳定 handle |
| P0 | query 默认 owned clone | cache hit 复制大型产品；首次存储也复制；API 被 `Clone` 反向塑形 | 默认 shared/arena handle；显式 owned extraction 只用于少数消费端 |
| P0 | loader/compiler/driver/reachability 有多套依赖收敛 | 重复分析、手写失效传播、正确性由同步约定维持 | 单一依赖引擎或至少单一事实注册表与统一 revision |
| P0 | `query_many` 不是调度系统 | 临时线程、嵌套并行失控、重任务仍串行 | 长寿命 worker pool、jobserver、任务预算、backpressure |
| P1 | 真实增量粒度仍是 module/program aggregate | 小改动仍物化、clone 和扫描整包产品 | item/body/mono-unit 作为一等产品和依赖节点 |
| P1 | 无跨进程 frontend/cache work product | CLI 冷启动每次重做 parser 至 backend lowering | fingerprint、序列化 query products、CGU/object cache |
| P1 | IR 层之间所有权和生命周期不清 | 多层同时存活、函数体随 module aggregate 复制 | 明确每层用途、消费点和释放/steal 规则 |
| P1 | codegen 以 source module 串行执行 | 无负载均衡、无 frontend/codegen overlap、无 unit cache | mono-item partition、CGU queue、缓存和流水重叠 |
| P1 | crate 碎片化与巨石文件并存 | 高扇出、glue 多、局部边界仍模糊 | 按稳定抽象重组 crate，按内部职责拆文件 |
| P2 | panic 作为 query 普通错误传输 | unwind 成本、边界模糊、并行错误恢复复杂 | 显式 cycle/error channel，panic 仅用于 ICE |
| P2 | 测试资源控制侵入生产入口 | 普通 `cargo test` 依赖隐藏全局限流 | 独立 compiler test harness、资源声明、进程/临时目录隔离 |
| P2 | perf 可观测性偏 query 名称计时 | 能看到“谁慢”，难看到 clone/alloc/RSS/复用原因 | 固定 workload + allocation/clone/query/cache/parallel 指标 |

## 4. 三者总体架构

### 4.1 Rust

简化后的状态流：

```text
rustc_interface
    -> Session / source map / crate loading
    -> GlobalCtxt + TyCtxt<'tcx>
         -> arenas + sharded interners
         -> query caches + dep graph + diagnostics
         -> HIR -> THIR -> MIR
         -> mono item collection / CGU partition
    -> async codegen coordinator
         -> LLVM work items / work products
    -> join_codegen -> link
```

关键不是所有东西都塞进一个结构，而是 `TyCtxt` 给编译器内部建立了共同的身份域、内存域和查询域。各 rustc crate 围绕这一内核提供算法，不需要每个 pass 自己携带完整 interner 快照或重新定义 module product。

### 4.2 Zig

简化后的状态流：

```text
Compilation
    -> cache / target / link / queues / diagnostics
    -> Zcu
         -> files + ZIR + incremental state
         -> InternPool (types, values, navs, tracked instructions)
         -> PerThread analysis context
         -> Sema -> AIR per function
         -> codegen task pool -> backend/link queue
    -> flush / saveState
```

Zig 的源码组织并不小，也存在很大的文件；但 `Compilation`、`Zcu`、`InternPool` 和 `AnalUnit` 的所有权关系明确。巨型文件是可维护性问题，却没有同时制造多份类型身份。

### 4.3 Nia

当前简化状态流：

```text
Driver
    -> LoaderDatabase / loader query graph
         -> source/syntax/AST/module graph
    -> CompilerDatabase / compiler query graph
         -> per-module interners and stage products
         -> provider discovery / semantic facts
         -> BodyIr -> FunctionBody -> BackendProgram
    -> driver provider-demand fixed point
    -> reachability fixed point
    -> monomorphization with cloned working interners
    -> backend lowering into full BackendProgram
    -> sequential LLVM module loop
    -> external linker
```

Nia 的局部模块大多可以独立理解，这是优点；但顶层没有与 Rust `TyCtxt` 或 Zig `Zcu+InternPool` 等价的 compiler kernel。结果是“上下文”以许多窄 facade、resolver closure、输入 DTO、interner snapshot 和 aggregate product 的形式散落。

## 5. 编译生命周期与状态所有权

### 5.1 Rust：复杂度集中，阶段消费关系明确

`rustc_interface` 是编排边界，`GlobalCtxt` 集中持有 session 生命周期内的数据。`TyCtxt<'tcx>` 是轻量、可复制的 handle，不是复制 context 本体。类型、常量、predicate 和列表进入 sharded interner；arena reference 将大量查询结果的复制降为 pointer-sized handle 传递。

Rust 也有全局上下文的代价：API 会普遍携带 `TyCtxt`，生命周期复杂，编译器内部耦合不低。但它把不可避免的耦合集中在“语义身份、查询、arena”这个真正共享的内核里，而不是让每个阶段重新包装一份共享状态。

### 5.2 Zig：顶层 state owner 与细粒度更新统一

`Compilation.update` 同时协调 source change、ZCU 更新、预链接任务、codegen/link queue 和最终 flush。`Zcu` 的 generation、outdated/potentially-outdated 集合、dependency、failed analysis 和 codegen task 都属于同一个更新生命周期。

Zig 对 LLVM 的增量能力仍有明确限制：当前 LLVM object 更新部分仍存在“only one codegen at a time”的约束，LLVM 也不支持 Zig 自托管 linker 那样的细粒度 incremental linking。重要的是，限制被明确编码在 backend feature 与 task ownership 中，而不是由测试线程数或调用者环境变量间接约束。

### 5.3 Nia：所有权分散到数据库、driver 循环和产品快照

`Driver::compile_with` 在持有 compiler mutex 的情况下执行 loader/compiler 交替更新：

1. loader `load_program`；
2. compiler `update`；
3. 查询 executable provider demands；
4. 把 demand 反馈给 loader；
5. loader 可能重新装载；
6. compiler 再次 update/check；
7. output 还可能产生新的 demand，然后继续循环。

这不是一个统一 query engine 内的自然依赖，而是两个数据库之上的手工 fixed point。`module_graph_state` 只提取若干 source identity/semantic/provider path 字段用于判定变化，意味着 dependency truth 分散在：loader cache、compiler cache、driver 的摘要和 provider 输出中。

**判断：P0。** 在这一层继续增加 provider-specific query，会让单个 query 看起来更纯，却不会减少顶层重复工作。首先要统一“模块为何存在、哪个事实使它可见、哪些语义节点依赖它”的事实模型。

这里需要区分 Nia 的设计潜力与当前实现。Nia 源于对 Kern 线性流水线的反思，并且没有 rustc 数十年的兼容包袱；从一开始追求显式产品、细粒度依赖和单一职责，确实有机会形成比 rustc 更清晰的目标架构。但“后发”“crate 更多”或“拆分更细”本身不构成优势。当前 Nia 的状态所有权、依赖图和性能模型还没有达到 rustc/Zig 的成熟度，而且实际效果已经部分违背最初的解耦目标。

健康的 Nia 不应是“没有中心”，而应是：集中语义身份、revision、存储、依赖规则和调度规则；分离 parser、type、trait、comptime、optimization 等算法；显式定义算法的输入、输出和所有权。中心内核与算法解耦并不冲突，稳定内核恰恰是外围模块能够真正解耦的前提。

## 6. 查询与增量架构

### 6.1 Nia 查询系统的实际成本模型

`crates/nia-query/src/lib.rs` 的通用契约要求：

```text
QueryKey::Value: Clone + Send + Sync + 'static
```

普通 `query()` 的 cache hit 在 `OwnedQueryOutput::cache_hit` 中执行 `value.as_ref().clone()`。首次计算后，为了把结果同时返回并存入 `Arc`，也会构造 `Arc::new(value.clone())`。只有 `query_shared()` 的调用者明确选择共享返回值时，cache hit 才只是 clone `Arc`。

当前源码中：

- 全 workspace 约 125 个 `QueryKey` 实现，其中 compiler query 约 115 个；
- compiler/loader 内约 442 处 `.query(...)`；
- 约 154 处 `.query_shared(...)`；
- 只有 9 处 `.query_many(...)`；
- compiler query 的 115 个 `Value` 声明中，直接写成 `Arc<...>` 的仅十余个；另有一部分通过 type alias 间接包装 Arc。

这说明共享返回不是默认语义，而是调用点和 query author 必须主动记住的优化。对于 `CheckedModule`、`ModuleGraph`、`TypeLowering`、`BodyIr`、`CodegenProgram` 等大对象，忘记使用 shared API 不只是微小开销，而会改变整个编译的 allocation 和 RSS 曲线。

目标 API 不应继续保留 `query()`/`query_shared()` 这种由调用者选择所有权语义的双轨。公开调用只保留一种 `get::<Q>(key)` 语义；value 位于 arena、typed slab、inline slot 或其他存储中的差异，由 query 声明和 runtime 内部决定。小值可以在内部使用 Copy fast path，但不能形成第二套调用 API。完成迁移的标准包括删除旧入口，而不是长期保留兼容层。

### 6.2 锁与身份开销

`QueryDbInner` 至少包含三个全局 mutex：

- type-erased caches map；
- query identity 到 slot 的 map；
- dependency graph。

每个 slot 还有自己的 `Mutex<QueryState<V>> + Condvar`。`slot_for()` 会经过 type id、key hash、erased key、全局 cache/slot 查找；dependency graph 在 forward/reverse 两个方向保存 cloned query identity，而 identity 又带 `Arc<dyn ErasedQueryKey>`。

这些设计单项都合理：type erasure 允许独立 query type，per-slot condvar 防止重复计算，双向图支持 invalidation。但组合后的问题是：**Nia 在每次查询访问上承担了通用并发依赖引擎的固定成本，却没有以足够细且足够共享的数据产品换来命中收益。**

实测 `array_list` 冷 check 中：

- `query.slot_for` 70,141 次；
- `query.record_dependency` 70,141 次；
- CPU 使用率约 98%，没有体现出与这些同步结构匹配的多核吞吐。

因此，单独把 mutex 换成更快的锁不是根治。若 Value 仍深拷贝、provider fixed point 仍重复、重任务仍串行，更低的锁开销只能改善次要比例。

这一层需要整体重新设计，但不应预设“无锁”是目标。Rust 的 sharded interner/query job 和 Zig 的 compilation/task runtime 同样使用锁与 atomic；真正应消除的是每次查询都经过多个全局共享结构的热路径。目标结构应采用 revision 单写者、revision 内 immutable value、按 query kind 分片的 typed table、worker-local dependency buffer、最小 per-slot state/latch 和统一 executor。只有 profiling 证明剩余锁竞争显著时才考虑 lock-free；为无锁引入复杂回收协议反而会损害可验证性。

### 6.3 invalidation：eager 清除，不是 red-green 验证

Nia 当前 invalidation 会沿 reverse dependency 做传递 BFS，清空受影响 slot 并删除边。它能保证显式失效，但不等同于 Rust 的 red-green：

- 没有稳定 dep-node fingerprint 来判断“上游执行了，但输出语义未变”；
- 没有跨进程反序列化 dep graph；
- 没有 work product reuse；
- query identity 包含当前进程对象，不能直接成为持久缓存 key。

这会产生两个后果：

1. invalidation 容易过宽，尤其在 module/program aggregate 上；
2. 即便 query 图很细，CLI 新进程也无法复用上一轮结果。

Nia 的增量目标至少应包含完整 red-green，而不是在现有 eager clear 上继续增加特判。实现顺序必须是 stable identity、统一 query API、输入/派生事实分类、稳定 fingerprint、red-green 验证、跨进程持久化；顺序颠倒只会给不稳定对象增加颜色状态。若后续有更精确的 domain-specific invalidation，它也应通过同一事实图表达，而不是另建一套依赖系统。

### 6.4 Rust 的 query kernel

Rust 的 query 不是 100 多个手写 `impl QueryKey`。查询声明生成 provider/vtable/cache plumbing；`TyCtxt`、dep graph、query job、cycle handling、stable hash 与 profiler 共享一个核心协议。

Rust incremental 的关键是：

- stable `DepNode`；
- result fingerprint；
- red-green marking；
- on-disk query cache 与 serialized dep graph；
- codegen work product；
- query job/latch 与编译器 thread pool 协作。

Rust 的系统远比 Nia 复杂，不应整体复制。Nia 最值得先借鉴的是两条不依赖规模的原则：

1. query value 的默认表示应当便宜共享；
2. query key 必须能映射到 session 内稳定、最好可持久化的语义身份。

Nia 原本追求的显式产品和较窄算法边界仍值得保留，并有机会比 rustc 的历史性全局 context 更容易解释；需要放弃的是用大量 snapshot、DTO 和 resolver callback 模拟解耦的当前做法。目标不是缩小愿景，而是为愿景补上能承载它的 compiler kernel。

### 6.5 Zig 的语义依赖

Zig 没有照搬 rustc query API。它通过 `AnalUnit`、`InternPool.Dependee`、outdated/potentially-outdated 集合和 generation 管理更新。依赖不是统一的“某 query 依赖某 query”字符串，而是按语义事实分类，例如：

- source hash；
- nav value / nav type；
- function inferred error set；
- type layout；
- struct default；
- namespace name existence；
- source/embed file；
- tracked ZIR instruction。

`TrackedInst` 允许旧 ZIR 与新 ZIR 建立稳定映射；无法映射时明确标记 lost。这样的依赖类型更难设计，但它准确表达“什么变化才需要重算”。

Nia 的语义模型总体更接近 Rust，但不少具体取舍接近 Zig；目前两类借鉴都停留在较初步的局部实现，没有形成统一内核。由于 Nia 的语言语义已基本稳定，未来 feature 增量很少，也明确不计划引入宏、传统 ADT 层或语言内建 async，这正是冻结 feature 扩张、集中完成架构升级的合适阶段。Kern 后期才加入 query 化，线性数据流已经难以重塑；Nia 应利用尚可重构的窗口，把现有语义回归测试作为契约，完成能够长期维护的基础模型。

### 6.6 对 Nia 的判断

Nia 应采用一个单一模型，而不是并列的“typed query 系统”和“semantic fact 系统”：**基于 immutable revision snapshot、typed key、canonical index 和 tracked dependency 的增量事实图。** Source hash、name existence、provider visibility、type layout、body check 和 mono item 都是同一图中的 typed node；它们只在 key/value 类型、计算函数和持久化策略上不同，依赖记录、cycle、revision、fingerprint 和调度规则完全一致。

具体取舍建议是：计算语义采用 Rust 式 typed demand-driven query 与完整 red-green；身份和物理存储采用更接近 Zig 的 typed index 与 dense store；生命周期和任务调度采用显式 compilation state，并吸收 rustc jobserver/CGU 的资源管理经验。这些不是三套拼接的架构，而是一个事实图模型下的不同实现层。

该内核应先写出可检查的不变量：同一 key 在一个 revision 至多有一个有效 value；green 节点的 tracked dependency fingerprint 全部匹配；mutable input 不得绕过 dependency recorder；cycle 是显式结果；执行顺序不改变 value 与 diagnostic；stale ID 不得跨 generation/session；incremental 结果必须等价于 clean recomputation。并发 slot 状态可用模型检查，增量正确性用随机输入变更和 clean/incremental 差分验证，为后续更正式的验证保留清晰边界。

这意味着需要一轮彻底重构，并且每阶段都以删除旧模型为完成条件。无论具体实现借鉴 Rust 还是 Zig，都不能再留下互相绕行的双轨。

## 7. 类型、符号、ID 与内存模型

### 7.1 Nia 的 module-local interner 是当前最深的架构债务

Phase B 开始前，`InternedTyId` 包含 `TyInternerId + TyInternerIndex`，而 `TyInternerId::for_module` 直接来源于 `ModuleId`。第一实现切片已经删除 `for_module`，改为 `TypeStoreId + ModuleId`，因此相同 `ModuleId` 的不同 compiler session 不再产生可误认的 handle；但 module shard 和 `TyInternerIndex` 仍存在，尚未成为最终统一 `TyId`。`TyInterner` 本身仍是可 clone 的 Vec/hash interner。多个后续阶段持有各自 interner，跨模块或跨阶段时仍通过 `try_import_type_into` 按 `TyKind` 递归重建类型。

实际代码中广泛存在：

- `module.interner = ...clone()`；
- `working_interners_by_module`；
- `comparison_interner`；
- `input_type_interner_snapshots`；
- `ProgramFunctionBodyInterners`；
- monomorphization 输出再携带 `HashMap<TyInternerId, TyInterner>`；
- backend module 再内嵌完整 `TyInterner`。

这不是“clone 稍多”的局部问题，而是 identity model 有两层含义：同一结构类型在不同 interner 中可能有不同 ID；某个 ID 的解释依赖它对应的快照。API 因此必须同时传 ID 和能解释 ID 的 interner。

这也解释了为什么 interner seed、backend snapshot、body seed 之类遗漏会产生难定位回归：每增加一个阶段产品，就增加一条必须保持同步的隐含不变量。

### 7.2 Rust 与 Zig 的差异

Rust 的 `Ty<'tcx>`、`GenericArgsRef<'tcx>` 等是统一 `TyCtxt` 中 arena-interned 的紧凑 handle。不同 query 不复制 interner，类型结构的共享由生命周期保证。interners 使用 sharded hash map，初始容量还根据 perf 实测调优。

Zig 的 `InternPool.Index`、`Nav.Index`、`TrackedInst.Index` 等是 `Zcu` 内统一的紧凑 ID。InternPool 采用 packed arrays、free list、per-thread locals 和 sharded mutation。它比 rustc 更显式地管理索引和数据布局。

对 Nia 而言，typed numeric index 比把 Rust arena reference/lifetime 扩散到整个编译器更合适。它适合 dense Vec/SoA、序列化、dependency key、统计和状态建模，也能把物理布局与 API handle 分离。但它不能只是裸 `u32`：不同 ID 必须是不可混用的 newtype；append-only store 不复用 index；会回收的 store 使用 generation；debug build 检测跨 session/stale handle；持久缓存保存 stable key，而不是本次 session 的 local index。

两者具体实现不同，但共同点是：**语义 handle 可以在整个 compilation unit 中直接比较和传递，不需要每个阶段携带“解释这个 handle 的快照”。**

### 7.3 Nia 应当采用的目标

建议建立 session-owned `SemanticContext`（名称可另定），至少拥有：

- type interner；
- const/value interner；
- symbol/string interner；
- stable source/module/definition identity 映射；
- target/layout context；
- arena 或分代 storage；
- query/dependency runtime 的入口。

`InternedTyId` 应成为 session 内统一 handle。模块归属是类型或 definition 的 metadata，不应决定 interner 空间。对于 LSP/daemon 长寿命场景，需要 generation/epoch 或 stable key 到 session-local index 的映射，避免 stale handle；不应靠复制整个 interner 隔离 revision。

这不等于引入 `'tcx` 到所有 public API。Nia 可以选择 Zig 风格的显式 index + owner，或 arena handle + session token。重要的是统一身份，而不是语法形式。

Phase B 开始时进一步核对现有创建路径后，需要把 identity contract 收紧为以下规则：

1. `TyId` 是 compilation-session local typed index，不携带 module owner；primitive/structural type 本来没有源码模块归属，nominal type 通过 `TyKind` 内的 stable definition identity 表达来源。
2. session type store append-only，已发布 slot 在 session 生命周期内 immutable 且永不复用。revision 改变 syntax/def -> type 的 fact，不重编号整个 type store；否则 incremental query 无法保留稳定 handle。
3. 不同 compiler session 由 store identity 隔离，store API 必须拒绝 foreign handle。当前 owned query product 尚不能用 Rust lifetime 静态限制时，以该检查作为 stale-handle 边界；session drop 后 handle 不具备可解释性。
4. 跨进程/跨 session 使用独立 `StableTyKey`：它由 stable definition/source identity 与结构数据 canonicalize，只在 persistence 边界映射为 local `TyId`，不能替代热路径 index。
5. type store 集中 ownership、canonicalization 与同步策略，但算法 crate 只能通过 typed API 查询/intern，不能获得可任意修改其他语义状态的 global world table。
6. 迁移从 type lowering 开始，依次经过 normalize、trait/body、layout、mono/backend；每个域迁移完成时删除该域的 snapshot/import 路径。adapter 只存在于“已迁移前缀 -> 下一个未迁移域”的单一边界，不提供 module-local/session-global 两套公开选择。

这也排除了一个表面简单的方案：只给旧 `TyInternerId(ModuleId)` 叠加 revision generation。declaration/full/signature type lowering 原先会分别从零构建同 module interner，正是依赖相同 module-derived id 和 prefix 假设才可互用；先强化 id 而不先统一 ownership，只会把隐式耦合变成大面积不兼容。当前实现先让 `CompilerContext` 持有唯一 `TypeStore`，四类 type lowering 在同一 store 的 module shard 内串行 canonicalize/append，再返回 snapshot 供未迁移下游使用；这样 store identity 与 append-only 规则已经真实生效，而不是 generation wrapper。规范已同步到 `docs/architecture.md`；统一 index 和 snapshot 删除完成前仍不得把 Phase B 标为完成。

### 7.4 Definition 稳定性

Nia 的 `ModuleId(u32)`、`DefId(u64)`、`SourceId(u32)` 当前主要是装载/收集过程中的编号。源码中没有与 Rust `StableCrateId + DefPathHash` 或 Zig `TrackedInst` 等价的完整跨 revision identity 层。

建议区分：

- `LocalModuleIdx` / `LocalDefIdx`：本次 session 的紧凑索引；
- `StableModuleKey`：canonical source/package identity 的 hash；
- `StableDefKey`：owner path、item kind、name/disambiguator 的稳定 key；
- syntax-local node identity：用于局部重解析和诊断，不直接承担跨版本语义身份。

没有这一层，on-disk query cache 只能缓存文本级/文件级结果，很难安全缓存类型、trait 或 mono products。

## 8. Source、Syntax 与模块装载

Nia 的 syntax 层有值得保留的基础：green/red lossless tree 和 partial reparse 能支持编辑器与长寿命 session。问题在于普通 CLI 冷编译中，这些能力还没有通过跨进程 cache 转化成收益。

当前 source 侧的若干成本：

- `SourceDatabase`/`SourceTable` 基于 `Arc<Mutex<HashMap<...>>>`；
- source 读取常返回 cloned `SourceFile`；
- `SourceIdentity`/`SourcePath` 包含 owned `String`，作为 query key 和 graph identity 时会重复 clone；
- loader 从 syntax tree 再构建 owned AST 和 `NodeOriginTable`；
- loader 与 compiler 各自维护 query graph。

Rust 的 parser/HIR 并不以 lossless tree 为核心，因此不能直接作为 Nia syntax 设计模板。Zig 的 AST -> ZIR 更接近“把 source 迅速降到紧凑、可缓存的语义前 IR”。Nia 可以保留 lossless tree，同时增加一个紧凑 item/body syntax product：

- item header、name、generic/where、body range 分离；
- body 延迟解析或至少延迟构建 owned AST；
- source path/string intern；
- parse product fingerprint 可序列化；
- module discovery 成为统一依赖事实，而不是 driver 回调循环。

## 9. 语义分析、Trait 与 Comptime

Nia 已经把 type lower、type resolve、normalize、trait solve、body check、comptime check 等职责拆开，这比 Kern 的粗阶段更利于演进。问题是拆分主要发生在 crate 和数据产品层，公共语义内核仍不够稳定。

典型症状包括：

- 每个阶段输入结构携带大量 map、interner、normalization、signature、trait index；
- callback resolver 被用来跨边界查询类型别名、trait、extension 或 provider；
- module-level facts 在 program-level index 与 checked module 之间重复聚合；
- 同一语义问题可能分别由 signature、body、executable facts 和 backend lower 再解释；
- diagnostics 随各阶段产品一起移动和合并。

Callback 本身不是坏设计：它可以隔离依赖、方便测试。但当 callback 的主要作用是弥补“被调函数没有统一 semantic context”，并且同一 resolver 在多处出现新旧 API 双轨时，它就变成缺失内核的信号。

Rust 把 trait solving、normalization 等通过 `TyCtxt` 和 canonical query 接入统一类型身份。Zig 的 Sema 更单体，但所有操作都围绕 `Zcu.PerThread + InternPool`。Nia 更适合 Rust 式的模块化算法，但应采用共同 context/handle，而不是继续扩大输入 DTO。

建议为语义层定义少量稳定 facade：

- `DefinitionStore`：definition/header/visibility/owner；
- `TypeContext`：intern、kind、normalize、layout request；
- `TraitContext`：impl candidates、goal solve、projection；
- `BodyContext`：local/source/origin/body facts；
- `DependencyContext`：记录语义事实读取。

这些 facade 都应是轻量 handle，引用同一 session storage；不应各自拥有 cloned program snapshot。

Facade 也不能退化成新的 service locator。`CompilerSession` 负责拥有 store；具体算法只获得完成任务所需的 capability view，例如 `BodyCheckCx` 只暴露 definition/type/trait/body dependency 能力。普通语义读取不再通过 resolver callback 注入；callback 只保留给真正的策略差异，例如 target-specific cost model。模块边界必须通过单向依赖和明确数据所有权成立，而不是靠 crate 数量制造表面隔离。

## 10. IR 分层与中端

### 10.1 Nia 当前 IR

Nia 的主要路径可概括为：

```text
owned AST / semantic tables
    -> BodyIr (typed tree, per-module map + TyInterner)
    -> FunctionBody (CFG blocks, but expressions仍是 nested owned tree)
    -> function optimization
    -> BackendModule/BackendProgram
    -> LLVM IR
```

`BodyIr` 是 `Clone + PartialEq` 的完整模块产品，包含 `TyInterner`、所有 function bodies 和 global init。`FunctionBody` 同样是深层 owned 结构，locals/scopes/blocks 用 Vec，但 block 内的 `FunctionExpr` 仍大量使用 `Box`/`Vec` 递归树。

`BackendModule` 再聚合：

- interner；
- comptime facts；
- 所有 layouts；
- structs/unions/enums；
- globals/global instances；
- functions/function instances；
- vtables；
- generic instantiations。

并且整个 `BackendProgram/BackendModule/BackendFunction` 体系普遍 derive `Clone`。这使 backend IR 既承担 codegen 输入，又承担 program snapshot/inspection/debug output，容易成为内存高水位中心。

### 10.2 优化层

`nia-function-opt` 的 pass 对 `FunctionBody` 原地修改，这是正确方向。但 `FunctionOptInput` 按值拥有 `FunctionBody`，其上游要么转移、要么先 clone；在通用 query cache 语义下，常见结果仍是“从缓存 clone 一份 body，再原地优化”。

当前 pass 以多个顺序扫描为主。对小函数可接受；对大函数，nested expression tree 会让 dataflow、use-def、dominance 等优化难以高效扩展。后续若要增加更强中端，不应继续在当前表达式树上堆 pass；应先决定 Function IR 是：

- 面向结构化 lowering 的临时 CFG；还是
- 面向优化的正式 MIR。

若是正式 MIR，建议把 expression 拆成 instruction/value arena，用紧凑 index 引用，建立 use-def 和 block predecessor/successor。若只是临时 CFG，则应尽快消费并释放，不应随 BackendProgram 长期保留。

现有架构文档和实现给出的职责实际上较明确：`BodyIr` 是 source-shaped checked runtime body，文档明确说明它不是 optimization MIR；`FunctionIr` 才是包含 block、terminator、scope edge 的 CFG-shaped backend function boundary；`FunctionOpt` 不是第三层 IR，而是直接作用于 `FunctionIr` 的 pass pipeline，包含 CFG cleanup、copy/constant propagation 等局部优化。因此早期“两个部分共同承担 MIR 职责”的理解在概念上有对应关系，但当前不应把二者视为同一种 IR 的两个容器。

Nia 不需要采用 Rust 的 HIR/THIR/MIR 命名或固定层数。可以保留更细粒度 query product 配合处理的独立设计，但应把契约收紧为：`CheckedBody(DefId)` 是不可变、source-shaped、可缓存产品；`LoweredFunction(MonoItemId)` 是 indexed CFG、可独占优化和消费的产品；`StaticInit(DefId)` 独立于函数 IR。`FunctionOpt` 只是 `LoweredFunction` 的 pass manager。是否把 value expression 进一步改成 indexed instruction arena，取决于 Nia 是否建设更强中端；无论选择哪条路线，都不能继续深 clone 或随完整 BackendProgram 长期保留。

### 10.3 Rust 的 HIR/THIR/MIR

Rust 不保证每层都小，但层次用途明确：HIR 反映语言级结构，THIR 服务 pattern/type-driven lowering，MIR 是控制流和优化/codegen 的核心。大型 MIR 在若干阶段通过 `Steal<Body>` 转移所有权，避免为了 query immutability 复制整个 body。

`Steal` 不是可直接照搬的万能工具：它会限制 query 调用顺序和可重复读取。但它体现了一条重要原则：**大型单消费者 IR 不应因为查询缓存抽象而被迫 Clone。**

Nia 最初以 query 化的细粒度 IR/product 协作替代传统整阶段 HIR/MIR，这个方向本身没有问题。当前执行变得奇怪，是因为查询粒度与数据所有权没有对齐：query 很细，结果却仍按 module/program 聚合；IR 分层很多，类型身份和生命周期却靠 snapshot/clone 传递。重构目标应恢复“按事实和 body 计算”的初衷，而不是为了模仿 Rust 改名。

### 10.4 Zig 的 ZIR/AIR/MIR

ZIR 是 AstGen 生成的紧凑、可追踪中间表示；Sema 按分析单元产生 AIR；AIR 的所有权可以直接交给异步 codegen task，task 完成后释放。后端若需要再生成 target-specific MIR。

Zig 的 AIR 路径与 Nia 特别值得对照：`analyzeFuncBody` 产生 `air`，明确维护 `air_owned`，若 backend 支持 separate thread 就把 AIR 所有权交给 codegen task；link queue 形成 backpressure。这同时解决了生命周期、并行和内存峰值。

### 10.5 对 Nia 的目标建议

建议把 IR 产品改为按 body/item 存储：

- `TypedBodyId` -> arena/Arc 中的 typed body；
- `MirBodyId` -> 可独占消费或版本化的 MIR；
- `MonoItemId` -> substitution 后的实例；
- `CodegenUnitId` -> 一组 mono items；
- module/program 只保存索引与摘要，不内嵌所有大对象。

每层都要写出：生产者、允许的消费者、是否可缓存、是否可序列化、何时释放、是否需要可变优化。没有这份 ownership contract，就不应再新增 IR 层。

## 11. Monomorphization 与 Reachability

Nia 的 monomorphization 会为每个模块 clone `TyInterner` 到 `working_interners_by_module`，然后在多个 map 中跟踪 substitutions、type instantiations、source edges、effective generics、symbols。输出又携带所有 working interners。

这套实现功能上已经不简单，但结构上有三个问题：

1. mono identity 仍依赖 `arg_module_id + module-local type IDs`；
2. collection 不是统一 dependency graph 中的 item traversal，而是先聚合 module inputs；
3. interner clone 把“发现实例”和“构造新的类型身份空间”绑定。

`nia-executable-reachability` 还有自己的循环：按当前 reachable modules 收集 body refs、traits、generic instances 和 fact owner modules，直到 change key 不变。incremental variant 仍通过多组 set 和重复的 current module materialization推进。

再叠加 driver provider fixed point，Nia 至少有三类收敛循环：模块/provider、语义 reachability、mono/backend foreign refs。它们不是错误，但没有统一 worklist 与依赖事实，因此可能重复扫描同一 module/program aggregate。

Rust 先收集 `MonoItem` 使用图，再 partition 到 CGU；Zig 则以 `AnalUnit`/function/nav 为单位把 Sema 与 codegen 更新串起来。Nia 建议统一为一个显式 work graph：

```text
root item
  -> referenced def / vtable / global / mono request
  -> normalized MonoItemKey
  -> owner-independent queue dedup
  -> per-item lowering
  -> CGU assignment
```

`MonoItemKey` 应只依赖统一 type/const handles 和 stable def identity，不应需要复制 source module interner。

## 12. LLVM Codegen、Link 与工作产品

### 12.1 Nia 当前实现

`emit_llvm_ir_with_options_inner` 和 `emit_native_objects_inner` 都：

1. 为完整 BackendProgram 建 `ProgramIndex`；
2. 验证完整 program；
3. 顺序遍历 `program.modules`；
4. 每个 module 创建新的 LLVM `Context`；
5. 创建 `ModuleCodegen`；
6. 完成 IR/object emission 后再处理下一个 module。

优点是实现清楚、module 隔离天然适合未来并行。缺点是当前没有：

- CGU partition；
- module codegen 并行；
- frontend 与 LLVM 优化重叠；
- module/object work product cache；
- 基于成本的负载均衡；
- queue backpressure；
- link 与 codegen 的统一任务协调。

source module 也不是理想 codegen unit：一个大 module 会成为串行长尾，很多小 module 又增加 LLVM context/module 固定开销。

与前端已有但失衡的设计不同，codegen 调度、CGU 和 work product 基本尚未形成完整架构，这是面向第一梯队目标的重大缺口。它不是当前 7 秒 `check` 的直接根因，但决定完整 build 的多核利用、内存上界和增量收益。该部分必须在 stable mono identity、统一 executor 和 IR ownership 建立后立即推进，不能用并行包装当前 source-module loop 代替设计。

### 12.2 Rust

Rust 把 mono items partition 为命名 CGU。CGU 是 LLVM codegen/optimization 和 incremental work product 的单位。async codegen coordinator 接收 frontend 产生的 module，按估计成本排队 LLVM work item；队列过深时会阻塞 frontend，避免大量 LLVM module 同时占内存。`join_codegen` 明确等待 ongoing codegen，再进入 link。

这一设计同时处理：

- 多核利用；
- LLVM 高内存任务限流；
- incremental object reuse；
- frontend/codegen overlap；
- ThinLTO/FatLTO 的后续任务。

### 12.3 Zig

Zig 对函数分析后创建 codegen task，并进入 link queue。AIR 可以转移给 task；queue 积压会阻塞生产者。自托管后端可以函数级更新；LLVM backend 受 LLVM object 模型限制更粗，但限制集中在 backend capability 上。

### 12.4 Nia 的演进顺序

不能直接把当前 module loop 包进 rayon 就结束。正确顺序是：

1. 建立稳定 `MonoItemKey`；
2. 把 backend lowering 从完整 module 变成 per-item/per-unit；
3. 定义 CGU partition 与 deterministic naming；
4. 引入持久 worker pool 与内存权重；
5. 让 codegen queue 与 frontend overlap；
6. 保存 CGU fingerprint/object work product；
7. 最后再考虑 ThinLTO 等高级策略。

否则并行只会同时 clone 多份 BackendModule 和 interner，降低 wall time的同时把 RSS 推得更高。

## 13. 并行调度与资源模型

### 13.1 `query_many` 的问题

当前 `query_many` 每次调用会创建局部队列和 mutex，clone 当前 thread-local query stack，用 `std::thread::scope` 创建 1–4 个 OS thread，结束后 join、合并 dependency set 并排序结果。

这适合验证并行查询正确性，不适合作为编译器调度器：

- 线程创建成本在每次调用重复；
- nested `query_many` 没有全局预算；
- 任务不知道成本和内存；
- backend lowering/codegen 不在同一个 pool；
- query slot 的 mutex 与临时队列 mutex 叠加；
- `NIA_QUERY_THREADS` 只能调参数，不能建立调度模型。

### 13.2 测试 permit 暴露了生产资源模型缺失

当前维护已把 `nia-test-support` 的进程内 `ResourcePool` 改为自动跨进程协调：每个 compiler unit 按 1.5 GiB、系统预留 1 GiB，容量取 CPU 与 `min(system memory, cgroup limit)` 的较小约束并最多为 8；build test 申请两个 unit。workspace 默认通过标准 `RUST_TEST_THREADS=2` 保守运行，高容量机器可显式覆盖该标准变量。Linux 使用 PID + process start time 回收崩溃遗留 slot，无法可靠判活的平台按 age 回收；无法取得内存上限时重编译测试退化为串行，而不是仅按 CPU 放大并发。`CompilerDatabase` 的公开 check/codegen 方法仍在 `#[cfg(test)]` 下直接获取 permit。

这里不应把 WSL、日常物理开发机和租赁开发机建模为三类 profile。正确的配置维度是进程实际可用的 CPU affinity/quota、cgroup/VM 可见内存和用户显式覆盖：WSL 走 Linux 路径并看到其 VM 资源，容器或受限租赁机采用 cgroup 上限，裸 Linux 开发机采用系统资源。Rust 仓库本身主要通过 `x.py`/bootstrap 统一 suite、jobs/jobserver、compiletest 并发和 CI 机器配置，而不是为机器类别维护不同测试语义；Nia 要吸收的是这个“统一调度入口 + 可覆盖资源预算”的分层。由于 Nia 仍要求根目录无参数 `cargo test` 成为可靠入口，当前轻量的自动探测是必要兼容层，但不应演变成第二套长期调度器。

分组实测验证了内存权重的数量级：`nia-compiler-query` 全组峰值约 866 MiB，`nia-driver` 483 个测试在两线程下峰值约 1.46 GiB，`nia-cli` 全套集成/执行测试峰值约 1.69 GiB。因此 1.5 GiB/unit、CLI 同时最多两个普通 compiler unit 是有测量依据的保守值，而不是仅按 CPU 猜测。

这个措施能防止 CI/OOM，是合理的临时保护；但它仍说明资源约束在测试外层，而不是编译器内部：

- 一次编译内部可能开临时 query threads；
- 多个 libtest case 又并发启动多个编译；
- LLVM/link 子进程不与 query threads 共享预算；
- 生产入口因 `cfg(test)` 改变调度语义。

它使无参数 `cargo test` 可以成为当前验收入口，但不是最终架构。长期仍应由 compiler executor 统一内部 query/body/LLVM 资源，测试 harness 只声明 session/step 权重。

### 13.3 目标调度器

建议由 session/driver 构造一个长寿命 executor：

- worker 数来自 jobserver/available parallelism；
- 所有 query fan-out、body check、mono lower、LLVM CGU、link prework 进入同一预算系统；
- 任务带 CPU cost、memory weight、blocking/external-process 属性；
- 支持 work stealing，但重 LLVM job 有独立 semaphore；
- nested parallelism 复用当前 executor，不创建新线程；
- queue 提供 backpressure；
- 测试 harness 只限制“同时多少 session”，不修改 compiler 内部行为。

Zig build 的 `Step.max_rss` 和 rustc async codegen 的 queue/token 都表明：编译器调度不能只按 CPU 核数，内存是第一等资源。

## 14. 诊断与错误边界

Nia 的各阶段普遍把 `Vec<Diagnostic>` 放入结果结构，program/check/backend 再 extend/clone/排序。优点是函数式、易测；缺点是 diagnostics 与大型产品绑定，query cache 会把错误集合一起复制和保留。

更严重的是 query 错误边界：`query()`/`query_shared()` 会把 `QueryError` 用 `panic_any` 抛出，执行器 `catch_unwind` 后识别 payload，`CompilerDatabase` 的多个公开入口再次 `catch_unwind` 并 downcast。cycle/invalid input 作为预期控制流穿过 panic 机制。

这会混淆三类事件：

- 用户错误/普通 diagnostic；
- query cycle 或依赖失败；
- compiler ICE。

Rust 内部也会 panic/bug，但 query cycle recovery、diagnostic context、fatal error 和 ICE 有专门通道。Zig 使用 error union、failed analysis map 和 `ErrorBundle`，错误归属到 analysis unit。

建议：

- provider 返回 `QueryResult<Value>` 或由 query runtime 维护显式 failure state；
- cycle error 与 poisoned/ICE 分开；
- diagnostics 存入 session diagnostic store，query product 只持 compact `DiagnosticId`/bundle handle；
- 并行任务合并 diagnostics 时按 stable source key 排序；
- panic 仅处理 invariant violation/ICE，不承担普通 query 控制流。

## 15. Crate、文件与 API 组织

Nia workspace 当前有 64 个 crate，Rust 源码约 22.7 万行（包含测试）。这个规模本身并不离谱；问题是两个相反现象同时存在。

### 15.1 过度 crate 化

直接生产依赖扇出很高（这里只统计各 manifest 的 `[dependencies]`，不把 `[dev-dependencies]` 混入）：

- `nia-compiler-query` 直接依赖 46 个 workspace crate；
- `nia-backend-lower` 28 个；
- `nia-body-check` 30 个；
- `nia-codegen-llvm` 19 个；
- `nia-driver` 13 个；此外还有 10 个 workspace dev dependency 用于集成测试。

`nia-compiler-query` 已经接近“把所有 crate 再连回来”的集成层。大量小 crate 并未形成窄、稳定的抽象，反而让改动需要新增输入结构、provider function、query key 和 glue。

### 15.2 crate 内仍有巨石

较大的文件包括：

- `crates/nia-compiler-query/src/query/mod.rs`：约 7,682 行，且混有大量内嵌测试；
- `nia-executable-reachability`：约 4,183 行；
- `nia-body-check/src/lib.rs`：约 3,700 行；
- `nia-comptime-engine/src/eval.rs`：约 3,396 行；
- `nia-body-check/src/bir.rs`：约 3,096 行；
- `nia-trait-solve/src/lib.rs`：约 3,067 行；
- `nia-type-lower/src/lib.rs`：约 2,809 行；
- `nia-backend-lower/src/lib.rs`：约 2,708 行。

因此问题不是“文件大”或“crate 多”中的一个，而是**稳定边界没有落在真正共享的内核上**。crate 边界先增加了编译依赖与 API 搬运成本，内部职责仍未拆清。

Nia 当前常在文件变大后直接拆子模块，这只能改善导航。真正的架构边界必须回答：谁拥有数据、输入输出是什么、维护什么不变量、依赖是否单向。若多个子模块仍共同修改一个大 context 并互相调用内部实现，它们在架构上仍是一个巨石。

### 15.3 与 Rust/Zig 的正确对照

Rust compiler 也有大量 crate，但 `rustc_middle`、`rustc_query_system`、`rustc_interface` 等承担清晰的内核角色；query declaration 和 shared context 降低跨 crate DTO 重复。

Zig 的大文件较多，不是代码组织的理想模板；其优点是核心状态 owner 清楚。Nia 应借鉴其 ownership clarity，而不是把 crate 合并成一个巨型文件。

### 15.4 重组原则

建议未来按以下标准判断 crate 是否独立：

- 是否有稳定、可解释且较窄的 public abstraction；
- 是否能避免依赖 compiler session 的大部分内部类型；
- 是否值得独立编译/复用/测试；
- 是否打断依赖环或隔离 backend/host tool 边界。

仅表示一个流水线步骤、但需要二三十个内部 crate 的模块，通常更适合成为 compiler kernel 内部 module。相反，syntax、diagnostic format、target description、LLVM wrapper、linker invocation 等有清晰边界，适合保留 crate。

## 16. 测试体系对照

测试体系的第一轮修复已经移除正常运行所需的 Nia 私有限流/超时环境变量，并建立上述跨进程预算；测试结构仍能反映更深的架构问题。

2026-07-14 的一次全量运行期间 WSL 实例被宿主拆除并重建。重启后的 Linux 内核日志无法保留旧实例 OOM 信息；Windows 事件在终止前只记录到 `Tcpip` 4231（TCP 临时端口空间耗尽），没有标准 Resource-Exhaustion/OOM 事件。随后单独复跑当时停留的 `nia-build` 全组仅约 207 MiB并通过，因此不能把该重启归因于 Nia OOM，但这次事件促成了跨进程/cgroup 预算和分组峰值验证。

资源预算收紧后，根目录无参数 `cargo test` 已在同一 WSL 环境完整通过：CLI commands、native execute、LLVM/codegen、101 个 compiler-query、483 个 driver case 与全部 doc-test 均自然完成，未再使用 `--test-threads` 参数或 Nia 私有环境变量。这证明当前保护层可以作为可靠默认入口，但不改变 13.2 的判断：最终资源 ownership 仍应回到 compiler executor，测试层只声明 workload weight。

Nia 约有 1,600 级 `#[test]`，覆盖面可观，这是资产。主要问题是：

- 大量 compiler integration case 嵌在生产 crate 的 `src/tests` 或巨型 query 文件中；
- 每个 case 常创建临时工程并完整跑 compiler；
- libtest 默认并发与 compiler 内部并发相乘；
- 共享 `/tmp`、进程 ID/计数器命名、linker/环境变量容易形成隐含全局状态；
- `nia-test-support::compiler_permit` 侵入 compiler 公开入口；
- semantic unit tests、query invalidation tests、CLI process tests、LLVM/link tests 没有统一的成本分类与调度层。

Rust 并不是靠 `cargo test` 直接跑所有 rustc 行为测试。它有 bootstrap 驱动的 compiletest suite，将 UI、codegen、incremental、run-make 等按语义和成本分组，测试 harness 管理 revision、target、输出 snapshot、并发与工具依赖。Zig 也由 build graph 组织 behavior、compile errors、standalone、link、libc 等 suite，并能给 build step 声明 `max_rss`。

Nia 最终可以仍让顶层 `cargo test` 自然成功，但内部需要一个 compiler test harness：

- 将源码片段/fixture 编译测试集中为 data-driven cases；
- 一个进程/session 可批量运行便宜 case，避免每个 case重复冷启动；
- UI diagnostic 用稳定 snapshot；
- LLVM/link/execute case 明确标为 heavy，并由 harness 分配资源；
- 所有临时目录使用唯一 RAII fixture；
- 环境变量只用于测试目标本身，不用于让 harness 正常运行；
- 单元测试不获得全局 compiler permit，integration suite 才声明 session weight。

这部分应在 compiler 内部调度器建立后单独重构，否则测试 harness 仍要猜测单次编译的线程/RSS。

## 17. 冷编译实测

### 17.1 Nia

命令：

```bash
/usr/bin/time -v target/debug/nia --timings=detail check examples/04_array_list.nia
```

结果：

| 指标 | 值 |
|---|---:|
| wall | 7.39 s |
| user | 6.83 s |
| max RSS | 490,048 KiB |
| stage check | 7.226 s |
| `query.provider` | total 9.889 s / count 5,554 / max 1.807 s |
| `executable_provider_demands` | total 3.384 s / count 2 |
| `entry_checked_program` | 1.146 s |
| `query.slot_for` | 0.125 s / count 70,141 |
| `query.record_dependency` | 0.050 s / count 70,141 |

detail timing 的嵌套 total 不能直接相加，instrumentation 也会有额外开销；但 wall、RSS、query 次数和 provider demand 两次执行是有效信号。

另外两个冷进程：

| workload | wall | max RSS |
|---|---:|---:|
| `examples/00_minimal.nia` check | 2.98 s | 279,812 KiB |
| `examples/02_slices_and_strings.nia` check | 3.97 s | 334,112 KiB |

即便 minimal 也接近 280 MiB，说明固定 session/program materialization 成本已经较高，不只是 ArrayList workload 的 feature 复杂度。

### 17.2 Kern 参照

命令：

```bash
/usr/bin/time -v /root/project/kern/target/debug/kernc \
  --timings --emit-llvm=optimized \
  --runtime-entry none --library-bundle std \
  /root/project/kern/examples/collections.kn
```

结果：

| 指标 | 值 |
|---|---:|
| wall | 1.85 s |
| user | 1.62 s |
| max RSS | 117,188 KiB |
| compiler timing | 1.683 s |
| functions | 974 |
| MAST expressions | 25,766 |
| MIR instructions | 7,025 |
| pre-cleanup LLVM instructions | 41,547 |

它还完成了 LLVM codegen/optimization 并输出很大的 LLVM IR。两者不是 apples-to-apples：语言语义、标准库、检查深度和实现版本不同。但 Kern 做了更多后端工作仍约为 Nia ArrayList check 的 1/4 RSS、1/4 wall，足以排除“只是 Nia feature 更多”这一解释。

Kern 更快的结构原因很可能是：显式粗粒度阶段、紧凑连续数据流、少量通用 query dispatch/clone、较少跨阶段快照。它的缺点同样明显：粗粒度全量分析、增量弱、阶段耦合和扩展性问题。结论不是退回 Kern，而是 Nia 需要把 Kern 的紧凑数据流与现代增量内核结合起来。

### 17.3 阶段 A release workload 基线

现已增加 `tools/perf.py` 与固定 `benchmarks/`/examples workload。runner 先用显式 `perf-alloc` feature 构建一次 instrumented release compiler，再让 minimal、strings/slices、ArrayList、traits、comptime 和完整 emit-exe 各自在新进程运行，并汇总 schema-versioned JSON。普通 compiler build 不安装 counting allocator，日常编译与测试的 allocation hot path 没有额外原子检查；自定义或 `--no-build` compiler 缺少 instrumentation 时 runner 会直接拒绝，而不是记录伪造的零值。compiler 的同一 `nia-timing` collector 直接输出 wall/user/sys、max RSS、CPU utilization、stage/query timings 及 typed counters；driver 上报 query executions/hits、provider-demand rounds、checked/reachable bodies，LLVM 上报 unit/object-reuse，instrumented detail timing 还能汇总实际 query value clone count/bytes。这里没有通过脚本解析人类 timing 文本，也没有新增隐藏环境变量。

2026-07-14 在当前 WSL release build 上的一次 smoke baseline（单次样本，仅验证量级与管线，不作为跨机器阈值）：

| workload | wall | max RSS | query executions | checked/reachable bodies |
|---|---:|---:|---:|---:|
| minimal check | 0.003 s | 39 MiB | 76 | 1 / 1 |
| strings/slices check | 1.535 s | 323 MiB | 4,082 | 94 / 94 |
| ArrayList check | 2.370 s | 480 MiB | 5,554 | 144 / 144 |
| traits check | 1.460 s | 318 MiB | 3,920 | 85 / 85 |
| comptime check | 0.071 s | 42 MiB | 176 | 1 / 1 |
| ArrayList emit-exe | 3.699 s | 1,042 MiB | 6,239 | 152 / 183 |

emit-exe 生成 30 个 LLVM units，当前 object reuse 为 0，直接暴露了阶段 G 的缺口。runner 在 machine metadata 中记录 CPU affinity、cgroup CPU quota、system/cgroup/effective memory；它不建立 WSL、物理工作站和租赁机 profile。首个样本只建立了 value clone count，下面的第二个 instrumentation 切片继续补齐 allocation/clone bytes。

第二个 instrumentation 切片把成功 alloc/dealloc/realloc 次数和 requested bytes 接入同一个 detail collector，并按线程测量 owned query value clone 期间的真实 allocator traffic；它由 perf runner 的显式 `perf-alloc` build 启用，不改变普通 compiler binary 的 allocator。collector flush/JSON 序列化发生在计数停止之后。新的 release smoke 样本为：

| workload | Rust allocated bytes | query value clones | query clone bytes |
|---|---:|---:|---:|
| minimal check | 1.24 MiB | 126 | 0.16 MiB |
| strings/slices check | 1.31 GiB | 32,193 | 50.3 MiB |
| ArrayList check | 2.12 GiB | 49,947 | 99.1 MiB |
| traits check | 1.24 GiB | 31,242 | 46.8 MiB |
| comptime check | 364 MiB | 340 | 1.25 MiB |
| ArrayList emit-exe | 2.98 GiB | 56,310 | 116 MiB |

累计 allocated bytes 远大于 peak RSS，确认大量短命 heap traffic；query value clone 是明确且应由阶段 C 删除的成本，但只占 ArrayList/emit-exe 总 allocator traffic 的一部分，不能把全部 churn 归因于 query clone。interner/snapshot、body/product materialization 和临时集合仍需后续分项。allocation/clone bytes 已完成基础测量；同资源形状 median comparator 与宽松相对 guard 已具备，但仓库尚无可运行 Nia LLVM suite 的 CI 环境及 main-branch trend storage，因此阶段 A 仍不能关闭。

## 18. 性能根因树

```text
冷 check 7.39s / 490 MiB / ~1 core
|
+-- 高固定成本
|   +-- loader + compiler 两套 DB
|   +-- syntax -> owned AST -> origin tables
|   +-- program/module aggregate materialization
|   +-- minimal workload 仍约 280 MiB
|
+-- 重复工作
|   +-- driver provider-demand fixed point
|   +-- reachability fixed point
|   +-- mono/backend foreign-ref worklist
|   +-- eager transitive invalidation
|
+-- 复制与保留
|   +-- Query Value: Clone
|   +-- ordinary cache hit deep clone
|   +-- per-module TyInterner clone/snapshot/import
|   +-- diagnostics and layouts embedded in products
|   +-- BodyIr / FunctionBody / BackendProgram simultaneously live
|
+-- 并行收益不足
|   +-- only 9 query_many call sites
|   +-- query_many creates scoped OS threads per call
|   +-- backend lowering loops sequentially
|   +-- LLVM modules sequential
|   +-- no shared budget / queue / backpressure
|
+-- 无持久复用
    +-- no stable semantic key layer
    +-- no serialized dep graph/query cache
    +-- no CGU/object work product
    +-- each CLI invocation starts cold
```

其中 `slot_for` 自身 125 ms 并不是最大热点。更值得注意的是 70k 次访问所驱动的 provider、clone 和聚合工作。若只优化 hash/lock，很可能得到个位数百分比收益，却保留所有架构风险。

## 19. Nia 相对第一梯队实现的具体差距

### 19.1 已确认的核心缺陷

1. 没有单一 compilation-owned semantic identity domain。
2. 类型 ID 的可解释性依赖 module-local interner snapshot。
3. 通用 query API 要求所有 Value 可 Clone。
4. 普通 query hit 复制结果，不是共享 handle。
5. query identity/dependency 使用 type-erased heap identity，单次访问固定成本高。
6. invalidation 只有 eager transitive clear，没有 red-green fingerprint。
7. 没有跨进程 query/parse/semantic cache。
8. loader/compiler dependency graph 分离。
9. driver 手工执行 provider discovery fixed point。
10. reachability 与 backend foreign refs 又各自维护 worklist/fixed point。
11. query fan-out 用临时 OS threads，没有 persistent executor。
12. 内部并行没有 CPU/RSS/jobserver 统一预算。
13. semantic/body/backend 产品仍以完整 module/program 为主。
14. 大型 IR 没有明确 steal/ownership transfer/early free。
15. monomorphization clone working interners。
16. source module 直接充当 LLVM module，缺少 CGU partition。
17. LLVM codegen module loop串行。
18. 无 codegen work product cache 与 frontend/codegen overlap。
19. query cycle/error 使用 panic unwind 作为普通控制流。
20. diagnostics 与阶段 product 绑定并反复聚合。
21. crate 高度碎片化，但集成 crate 依赖几乎整个 workspace。
22. 大量手写 query/provider glue，API 迁移容易保留双轨。
23. 测试通过全局 permit 补偿 compiler 资源模型。
24. perf 指标缺 clone bytes、allocation、cache hit/reuse、peak live IR 等关键维度。

### 19.2 不能仅由当前证据断言的事项

以下需要专项工具验证，不能直接写成既定事实：

- 490 MiB 中每类对象的精确占比；
- interner clone 相对 provider computation 的具体 wall-time 百分比；
- 全局 query mutex 在高并发 workload 下是否已经是主要 contention；
- detail timing instrumentation 的精确放大倍数；
- 某一个 body-check/trait pass 是否存在算法复杂度退化；
- 把 module codegen 并行后可获得的真实 speedup。

建议用 heap profiler、allocation counters、clone instrumentation 和固定 corpus 验证，而不是凭直觉选择微优化目标。

## 20. 不建议采取的方向

1. **不要继续为每个语义事实新增一对 query key/provider，而不改变 Value ownership。** 这会增加 graph 节点和 glue，未必减少产品粒度。
2. **不要把所有大型 Value 机械包成 Arc 就宣布完成。** Arc 能止住 clone，但如果内部仍是整 module aggregate，invalidation 和峰值生命周期仍然过宽。
3. **不要用更多环境变量解决调度。** `NIA_QUERY_THREADS`、compiler check limit 等只能作为诊断开关，不能成为正常运行契约。
4. **不要直接把当前 backend module loop 并行化。** 先控制 interner/IR clone 和任务内存权重，否则测试 OOM 会更严重。
5. **不要追求“crate 数越少越好”。** 应合并没有稳定抽象的小 crate，同时保留 syntax、backend wrapper 等真正隔离边界。
6. **不要照搬 rustc 全套 incremental query。** Rust 的兼容/metadata/多后端复杂度远超 Nia；先实现 stable identity、单一 query 访问语义、fingerprint 和少数高价值持久产品。
7. **不要照搬 Zig 的巨型文件。** 借鉴其 state ownership、紧凑 ID 和 task transfer，而不是源码布局。
8. **不要退回 Kern 的全量粗流水线。** Kern 的性能证明紧凑数据流的重要性，不证明其扩展架构正确。
9. **不要为 clippy/test 通过添加 allow 或更多全局锁。** 如果 lint 暴露所有权、生命周期或 Send/Sync 设计别扭，应回到 API 模型修正。
10. **不要长期维护新旧 API 双轨。** resolver/type alias/provider API 迁移应以删除旧路径为完成条件，并通过依赖图和编译错误迫使调用点统一。

## 21. 建议的目标架构

建议目标不是一个“大重写”，而是明确最终形态，使每次迁移都删除旧路径。

```text
CompilerSession
|
+-- SourceStore
|   +-- interned paths/text handles
|   +-- syntax/compact AST products
|
+-- SemanticContext
|   +-- DefinitionStore + stable keys
|   +-- global Type/Const/Symbol interners
|   +-- target/layout context
|   +-- arenas / generations
|
+-- RevisionedFactGraph
|   +-- one typed get interface
|   +-- input and derived fact nodes
|   +-- fingerprints + red-green revisions
|   +-- persistent cache codec
|
+-- WorkExecutor
|   +-- jobserver CPU budget
|   +-- memory-weighted queues
|   +-- body/mono/codegen tasks
|   +-- backpressure
|
+-- ProductStores
|   +-- item signatures by DefId
|   +-- typed bodies by BodyId
|   +-- LoweredFunction by MonoItemId
|   +-- CGU/object work products
|
+-- DiagnosticStore
    +-- stable diagnostic IDs/bundles
    +-- deterministic merge/render
```

顶层 `Driver` 只负责：构造 session、提交编译目标、选择输出、等待任务、render diagnostic/link。它不应实现语义 fixed point。

## 22. 分阶段路线图

### 阶段 A：建立基线与防回归指标

在架构改动前固定 4–6 个 workload：minimal、slices/string、ArrayList、trait-heavy、comptime-heavy、完整 emit-exe。记录：

- wall/user/sys；
- max RSS；
- query executions/hits；
- cloned bytes/clone count（至少对 top-level products/interner）；
- allocations/allocated bytes；
- provider-demand rounds；
- reachable/checked body count；
- CPU utilization；
- LLVM units 和 object reuse。

Acceptance：基准命令不依赖隐藏环境变量，结果写 machine-readable JSON，CI 只做宽松 guard，perf runner 做趋势分析。

进展（2026-07-14）：固定六 workload、release runner、schema v1 JSON、进程资源、query/provider/body/LLVM counters、显式 feature-gated allocator traffic、query value clone count/bytes 已落地并完成全套 smoke baseline；普通 compiler 不承担 allocation instrumentation 开销，同机 comparator 会校验 OS/arch/CPU model/effective CPU/memory，并提供宽松相对 guard。阶段仍为进行中：仓库没有可运行 Nia LLVM suite 的 CI 定义与 main-branch trend storage，不能把开发机样本硬编码成项目阈值，也不能因比较工具存在就提前宣告 Acceptance 达成。

### 阶段 B（P0）：统一 semantic context 和类型身份

1. 定义 session-local typed index、跨 revision stable key、generation/reuse 规则及 stale-handle 不变量。
2. 引入 session-owned type store 与统一 `TyId`。
3. 为现有 module-local interner 建一次性迁移 adapter。
4. 逐域迁移 type lower -> normalize -> trait/body -> layout -> mono/backend。
5. 每迁移一域就删除对应 snapshot/import API。
6. 最终删除 `ProgramFunctionBodyInterners`、working interner copies 和跨 interner recursive import。

Acceptance：生产代码中不再出现跨 interner type import；backend product 不再内嵌 interner snapshot；同一类型 handle 可在一个 compilation session 的 module/stage 间直接比较；stale/local/stable identity 规则有自动化验证。

进展（2026-07-14）：identity/ownership contract、type lowering 与 normalization 生产路径的迁移切片已完成。`CompilerContext` 现在持有 compilation-owned `TypeStore`；declaration/full/signature/signature-comptime type lowering 与四类 normalization provider 全部在同一 append-only module shard 中 intern；`TyInternerId::for_module` 已删除，store identity 会拒绝不同 compiler session 的 foreign handle。Normalization 不再先 clone interner 再私有写入，也没有保留旧 API：唯一算法入口显式接收 mutable interner 与本次 lowering 的 input IDs，query provider 只负责提供 store transaction，因此 signature subset 不会误归一化 store 中其他 subset 的类型。自动化测试覆盖跨 update prefix/旧 slot 不变、跨 database 隔离、normalization 只追加且只处理显式输入，以及 signature/full 两种执行顺序共享 alias expansion ID。后端实例布局原先重新归一化后仍读取旧 interner，并分别维护 local/foreign 两条重复路径；现在它对既有 backend working interner 使用同一算法、统一读取归一化 snapshot，并合并为一条实例布局路径。过渡期 shard identity 紧凑编码为 64-bit，`InternedTyId` 为 16 bytes；该尺寸暴露的 `FunctionForHeader` 大小悬殊已通过只对 recursive condition 增加 ownership indirection 修正，没有 Clippy allow。上一切片相对 baseline 的单次六 workload smoke 全部通过 guard：ArrayList wall -2.7% / RSS +0.5% / allocated bytes +1.4%，emit-exe wall -0.4% / RSS +1.4% / allocated bytes +2.4%；这些单样本只用于排除明显回退，不宣称性能收益。本切片原样 `cargo test` 和严格 workspace Clippy 均通过，六 workload perf guard 也通过：相对 `type-store` baseline，ArrayList wall -1.26% / RSS +0.23% / allocated +0.001%，emit-exe wall -2.11% / RSS -0.015% / allocated -0.010%；其余 workload 均在宽松 guard 内，query executions 全部不变。Phase B 仍在进行中：当前 handle 仍包含临时 module shard，`TypeLowering`/`TypeNormalization` 仍为未迁移下游携带 snapshot，trait/body/layout/mono/backend 的 working interner 与跨 interner import 也仍存在。下一切片迁移 trait/body 的类型创建与 normalization 消费边界；不能用 type alias 或永久 adapter 假装统一 `TyId` 已完成。

### 阶段 C（P0）：重做 query value/storage 契约

1. 去掉通用 `Value: Clone` 要求。
2. 公开调用统一为一种 `get::<Q>(key)` 语义，调用者不能选择 owned/shared 路径。
3. cache/store 唯一拥有 value；arena ref、typed handle 或小值 Copy 是 runtime 内部 storage policy。
4. 把 module/program aggregate query 拆为 index + item/body handle。
5. 建立显式 declarative query registry，记录 key/value/provider/fingerprint/storage；代码生成只允许机械消除 glue，不隐藏依赖语义。
6. 删除 `query`、`query_shared` 等旧入口和兼容 adapter。

Acceptance：所有调用点使用同一查询入口；cache hit 不深拷贝；clone instrumentation 中 compiler product clone 接近零；旧 API 删除，不保留双轨。

### 阶段 D（P0）：统一模块/provider 依赖

1. 建立统一 revisioned fact graph；source hash、module existence、provider visibility、name existence 与普通 derived query 使用同一 node/runtime。
2. loader 与 compiler 共享 revision/dependency recorder，或成为一个 session DB 的不同 provider group。
3. 实现 stable fingerprint 与完整 red-green validation。
4. 把 driver provider-demand loop 转成 dependency-driven worklist。
5. 把 reachability/foreign-ref worklist接入统一 scheduler 与 key dedup。
6. 删除 `module_graph_state` 摘要式同步协议和 eager-clear 旧图。

Acceptance：一次冷 check 不再出现 driver 层重复 load/update round；provider 新增只注册 typed fact/provider，不修改 driver fixed point；green node 依赖 fingerprint 全部匹配；随机修改下 incremental 与 clean recomputation 等价。

### 阶段 E（P0/P1）：持久 executor 与测试资源模型

1. session 创建 executor，接入 Cargo jobserver（可先用固定 worker fallback）。
2. `query_many` 改为提交 task，不创建线程。
3. body check、program signature fan-out 和 reachability scan 接入 executor。
4. LLVM 重任务加入 memory semaphore/backpressure。
5. test harness 只控制 session 数，移除 compiler API 内 `cfg(test)` permit。

Acceptance：无 `NIA_QUERY_THREADS` 也能稳定运行；普通 `cargo test` 不需 `--test-threads`；单编译和多测试并发都不超预算。

### 阶段 F（P1）：IR ownership 与 item 粒度

1. 固化 `CheckedBody`、`LoweredFunction`、`StaticInit` 的正式职责与 ownership contract。
2. checked body、lowered function 改为 store + typed handle。
3. 对单消费者 IR 实现 owned extraction/steal 或 generation replacement。
4. backend lowering 改为 per-mono-item/per-CGU。
5. 让旧 IR 在消费后释放，测 peak live bytes。

Acceptance：BackendProgram 不再包含所有 function body 深树和 interner；peak RSS 显著下降；单 body 变更不重新 lower 无关 module bodies。

### 阶段 G（P1）：CGU、异步 codegen 与 work products

1. deterministic mono collection 与 CGU partition。
2. codegen task queue；frontend 与 LLVM overlap。
3. CGU fingerprint、object cache、incremental link inputs。
4. 记录 CGU reuse 与 invalidation reason。

Acceptance：多核 workload CPU 利用率明显提升；小改动只重建受影响 CGU；并行不会显著抬高 RSS。

### 阶段 H（P1/P2）：持久 frontend incremental

1. stable module/def key。
2. source/syntax/item signature fingerprint。
3. serialized dep graph 与高价值 query products。
4. cache schema/version/target/options namespace。
5. corruption fallback 与 correctness verification mode。

Acceptance：第二次无改动 check 接近 cache validation 成本；单文件 body edit 不重跑无关 module signature/trait/layout。

### 阶段 I（P2）：错误、诊断和工程重组

1. 移除 panic-based query error flow。
2. 建 diagnostic store/bundle。
3. 合并无稳定抽象的小 crate，拆分巨型内部文件。
4. 建 data-driven compiler test harness 与 suite 分类。
5. 删除旧 API、兼容 adapter、临时环境变量和测试 permit。

## 23. 风险与验证指标

### 23.1 最大风险

- 统一 interner 会触及几乎全部语义层，是最大迁移面；必须按域迁移并在每步删除旧路径。
- stable identity 设计错误会污染后续持久 cache；应先只用于进程内，再开放序列化。
- immutable query values 会暴露当前依赖可变 clone 的代码；不要用 interior mutability 普遍替代 clone，应区分 immutable product 与独占优化产品。
- executor 引入后可能暴露锁顺序/cycle bug；先用 deterministic single-worker mode 验证，再扩并行。
- CGU 会改变 symbol/internalization/link 行为；需专门 codegen-unit 和 ABI regression suite。

### 23.2 建议的架构守卫

- 禁止 compiler production crate 新增 `TyInterner: Clone` 使用；
- 禁止新 query value 是大型 owned aggregate；
- 禁止 query provider 直接创建 OS thread；
- 禁止 driver 新增 semantic fixed-point 分支；
- 禁止 backend module 内嵌 semantic store snapshot；
- 禁止测试依赖未登记的全局环境变量或共享目录；
- 所有兼容 adapter 必须有删除 issue/阶段，不允许永久双轨。
- query/fact graph 的并发状态机必须通过模型测试；增量结果必须持续与 clean recomputation 差分验证。

### 23.3 目标指标建议

不要过早承诺绝对数字，但可设置方向性 gate：

- minimal cold check RSS 先降低 30% 以上；
- ArrayList cold check wall 至少先回到 Kern 的 2 倍以内，再继续优化；
- ordinary query cache hit cloned bytes 为 0；
- provider-demand 编译轮次固定为 1 个统一 worklist 生命周期；
- `query_many` OS thread creations 为 0；
- 无改动二次 check 的 semantic/body/codegen execution 接近 0；
- 普通 `cargo test` 无参数稳定通过，不依赖隐藏 limit env；
- perf corpus 同时报告 wall、RSS 和 correctness hash，防止用内存换时间或跳过工作。

## 24. 最终判断

Nia 当前不是“实现质量落后一大截、需要全面重写”，也不是“只差几个热点优化”。它已经具备第一梯队编译器常见的许多组件：lossless syntax、typed query、trait/normalize/comptime 分层、function IR、reachability、monomorphization、LLVM backend、丰富测试。真正的问题是这些组件之间缺少一个统一、性能导向的 compiler kernel。

与 Rust 的差距主要在：统一 `TyCtxt` 式身份/arena、稳定 query/dep-node、IR 所有权转移、CGU/work product 和成熟调度。与 Zig 的差距主要在：`Compilation/Zcu` 式明确 state owner、紧凑 InternPool、semantic fact dependency、per-function AIR ownership 和 codegen/link queue。

Kern 的性能说明 Nia 在从粗粒度流水线走向细粒度架构时，出现了“复杂度先支付、收益尚未兑现”：数据更分散、查询更多、快照更多，但增量复用和并行吞吐没有同步建立。

因此下一阶段最重要的不是继续扩 feature，也不是局部清理文件，而是按以下不可颠倒的顺序修复基础：

1. 统一 session semantic identity；
2. 删除 query 默认深拷贝；
3. 合并 dependency/fixed-point 真相来源；
4. 建立持久、资源感知的 executor；
5. 把产品粒度降到 item/body/mono unit；
6. 再建设持久增量与 codegen work products；
7. 最后收束 crate/API/测试组织并删除所有临时双轨。

完成前四项后，Nia 的性能、回归频率、API 一致性和测试稳定性会同时改善；这也是判断重构是否触及根因的标准。若一次改动只让某个 benchmark 更快，却继续增加 interner snapshot、query Arc 包装、driver 特判或测试环境变量，它就没有朝目标架构前进。
