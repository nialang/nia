# Nia 编译器架构审查与重构路线图

> 审查日期：2026-07-14
>
> 性质：以 Rust/Zig/Kern 对照审查为基线，持续记录目标架构、分阶段重构计划与已验证进展；不比较语言 feature 数量
>
> 结论强度标记：**确认**表示可直接由当前源码或实测得到；**推断**表示有明确结构证据、但仍需专项 profiling 验证比例；**建议**表示目标架构判断

## 当前执行状态（2026-07-23）

本文件使用 `compiler-roadmap.md`，而不是 `plan.md` 或 `task.md`：它同时包含长期架构审查、目标模型、阶段路线图和滚动验证记录，不是一次维护任务，也不是执行完即可删除的短期计划。

完成度按各阶段的 acceptance 和旧数据流是否真实删除估算，不按提交数量、代码行数或进展段落长度计算。百分比只表示当前决策尺度，避免把尚未建立的 CI、executor、持久增量或 work product 算成“已有基础所以接近完成”。

**核心执行准则：Nia 不为旧实现保留兼容包袱。** 每个迁移域都以旧入口、旧身份、fallback、重复 truth source 和新旧双轨真实删除为完成条件；调用面大、旧产品仍被引用或迁移成本高都不是保留兼容层的理由。只有在 retirement 开始前已经由 query graph 外调用方取得的 immutable revision snapshot 才可以按其既有所有权继续存活；它必须与 current revision 隔离，旧 local handle 不得从当前 query graph 可达，也不得提供旧 revision lookup 或 history API，最后一个外部引用释放后由所有权自然回收。query cache、slot table、dependency graph 和内部 history store 都不算外部调用方；旧 revision entry 必须在 quiescence 后从这些 owner 中物理退休，不能以 green cache、调试追溯或兼容为由保留。任何临时 adapter 都只能位于单一、持续前移的迁移边界，并必须在对应阶段内删除，不能成为长期公开 API。

| 阶段 | 当前估算 | 判断 |
|---|---:|---|
| A 基线与防回归 | 约 85% | 七个workload、machine-readable指标、allocator traffic/current/peak-live、query/LLVM counters与同机guard已完成；multi-module backend workload强制验证finalization live window，baseline comparator同时守卫allocated与peak-live bytes。可运行LLVM suite的CI和main-branch trend storage未完成，但不阻塞当前进程内identity/query/fact graph临界路径。 |
| B semantic context / 类型身份 | 100% | session-wide canonical `TypeStore`、全 pass canonical read/append、显式 roots、跨 revision slot 稳定与跨 session 隔离已完成；module view/log、origin、snapshot/checkout、recursive import 和旧 identity 类型均已删除。 |
| C query value/storage | 100% | cache-owned `get/try_get/get_many`、无 `Value: Clone`、declarative registry 与 aggregate storage policy 已完成，owned runtime adapter 全部删除。 |
| C 后 ID/arena 专项 | 100% | ID-0 query dep-node arena、ID-1 source/syntax identity、ID-2 graph/fixture 收口与 ID-3 owner/index/generation module handle 均完成；构造、fork、stale lookup 与 local-slot 边界已有守卫。 |
| D 统一依赖图 | 100% | loader/provider update 已收敛为 current canonical demand snapshot、至多一个 pending additive transition 与 current revision graph query；compiler provider/reachability worklist直接读取 tracked facts并在 session transaction内收敛，Driver 只提交一次编译目标。loader/compiler typed DB 共享同一 `QuerySession`，aggregate snapshot、手写 diff/delta 回灌、Driver fixed point、reachability take/store 与 query 外 checked-module payload均已删除，随机 incremental/clean 差分守卫通过。 |
| C/D 后 revision retirement | 100% | source update 已成为单一 quiescent retirement transaction；旧 `SourceVersion` query、provider graph lineage 与 `NodeStore` revision shard 均从 current owner 物理删除。source edit与provider add/reset各100轮后live slots、edges、transition、active node revisions与locator payload有界，monotonic ID不复用，外部旧`Arc`只持有自身immutable shard。 |
| E executor / 资源模型 | 100% | session-owned persistent executor、按storage policy分离且共享同一batch runtime的`get_many`/`get_many_owned` fan-out、Cargo/GNU Make jobserver CPU 预算、LLVM memory backpressure 与 test/production API 同构均已完成；unit tests 不再获取全局 compiler permit，只有 integration harness 声明完整 session 权重。 |
| F IR ownership / item 粒度 | 100% | checked body、static init与lowered Function IR均已成为item-owned product；backend只借用query-owned function/static IR，无consumer的过渡store/ID、batch API与`BodyIr` backend入口已删除。monomorphization/backend aggregate已有production query owner，per-module frontend source-item与function-instance plan也已成为tracked semantic product；跨模块动态closure已收敛为deterministic iteration snapshot，source/function/global materialization的post-substitution refs与vtable discovery均进入closed delta，旧aggregate body/vtable重扫已删除。所有module现在只在全程序closure收敛后执行一次optimization、DCE、aggregate/layout completion；完整closed items由不可clone的`BackendItemPlanQuery`按值交给唯一finalization consumer。closed plan只执行一次closure，随后将不可clone的module payload直接发布到`BackendModuleItemPlanQuery(ModuleId)`独占槽并按值消费；没有`Arc`、side store或module级重算，消费后slot无payload且失效链完整。compiler以唯一owned readonly environment实现`BackendProgramFacts`，正式`BackendModuleFinalizationQuery`通过统一`get_many_owned`在persistent executor调度，独占结果按原position确定性合并。multi-module release样本的并发窗口峰值增量中位仅约1.10 MB，未抬高全程Rust heap峰值；deterministic source partition与owned LLVM task context现已在Phase G落地，Phase F ownership acceptance关闭。 |
| G CGU / 异步 codegen / work products | 约 97% | deterministic bounded source bucket、跨session stable CGU key、versioned CGU/link-result component fingerprint、owned readonly whole-program context、资源有界LLVM worker lanes、持久native object/link-result cache、typed incremental link inputs及两层精确invalidation reason已完成；代表性large workload已同时关闭CPU与RSS acceptance，frontend/LLVM尚未overlap。 |
| H 持久 frontend incremental | 约 0–5% | 只有局部 artifact cache 和进程内 query 复用，不具备 stable module/def key、序列化 dep graph 与持久 frontend product。 |
| I 错误、诊断与工程重组 | 约 10% | compiler API 与 unit-test helper 的隐藏 permit 已删除；panic-based query flow、diagnostic store、data-driven integration harness、外层 session weight替代方案和 crate/巨型文件重组仍未系统推进。 |

综合判断：**整份路线图按剩余工程复杂度加权约完成 93%，合理区间为 91%–95%；A–F 的P0 ownership/runtime基础已完成。** 已完成的是 type identity、query storage、统一 revisioned fact graph、session 内持久 query调度、进程级CPU/LLVM memory协调、item-owned backend IR、deterministic bounded source partition、stable CGU/link-result component fingerprint、真实per-unit LLVM task queue、持久object/link-result cache、typed incremental link inputs与两层精确invalidation reason。source module finalization及LLVM emission均通过同一persistent executor与资源预算；LLVM task共享唯一owned readonly whole-program index，独占LLVM context/target/output，并按partition顺序归并。Driver在对象写盘时保留stable key/fingerprint，Linker直接消费唯一ordered typed input product；Driver在临时object落盘前按完整link identity恢复最终结果，旧source/request manifest executable cache已删除。尚未完成的是frontend与LLVM overlap、source bucket拆分后的CPU/RSS专项量化以及持久frontend product；当前只有完整link-result复用，不应冒充partial relinking或profile-guided最终CGU模型。

ID/arena 专项、Phase D、Phase E 与 C/D 后 revision retirement 均已关闭。loader/compiler typed DB 共享同一 QuerySession dependency arena；Driver 不再实现语义 fixed point，compiler session transaction串行消费 provider/body/reachability worklist；所有 query batch共享进程 CPU预算，LLVM入口另受进程级内存预算约束，测试构建不再改变 compiler/LLVM public API语义。source replacement现在在一个quiescent transaction内原子修改source、失效current roots并退休query/node/provider owner；中央cache、slot、edge、provider event与node locator均不再保存历史revision。Phase F已建立checked-body/static-init item producer、per-function lowering、per-module frontend source-item/function-instance plan和backend aggregate production query owner，且按consumer审计删除无用途的body store/ID；backend不再接收`BodyIr`或完整`Monomorphization`，function/static payload只经调用栈借用索引进入materialization。post-substitution refs、vtable entries及其诱发边现在从同一closed delta进入deterministic closure，旧aggregate body/vtable重扫已物理删除；module optimization、DCE和layout completion也已移到全程序closure之后的唯一finalization边界。closed result现在由不可clone的`BackendItemPlanQuery`按值move进唯一finalization consumer，再由正式owned fan-out发布为module-keyed query payload；每个slot消费后只留依赖与统计，finalized aggregate/module也不再提供深`Clone`。module finalization拥有只读`Send + Sync` context、独占`Send` result和按原batch position稳定归并的task-shaped边界；compiler-side query product实现唯一`BackendProgramFacts`契约，正式module-finalization query通过`get_many_owned`进入同一persistent executor。七workload perf suite现在含multi-module backend路径及跨worker finalization live window，三次release样本的峰值增量中位约1.10 MB且没有创造全程新峰值。当前临界路径转为deterministic CGU partition；禁止增加clone fallback、恢复aggregate ownership或把source module直接冒充最终CGU。Phase A剩余CI/trend storage在托管环境和基线存储策略明确前不抢占该路径。

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

1. **会话级类型身份与 semantic product 已统一。** `InternedTyId` 是 session-wide 8-byte handle；所有 pass 通过 canonical store 读取、通过 append capability 发布。跨 interner import、paired product、snapshot/checkout、module visibility log 与 layout full-scan 均已删除。
2. **查询存储契约鼓励复制。** `QueryKey::Value: Clone` 是通用约束，普通 cache hit 深拷贝值；大产品又常由完整 module/program aggregate 承载。细粒度查询因此没有自动带来细粒度数据流。
3. **依赖图和 fixed point 分裂。** loader query DB、compiler query DB、driver 的 provider discovery 循环、reachability 自己的 fixed point 分别维护“什么依赖什么”。统一增量正确性只能靠多层同步约定维持。
4. **真实工作单元没有进入统一调度器。** `query_many` 临时创建 OS 线程，而 backend lowering、LLVM module codegen 等关键重任务仍主要串行；没有持久 worker pool、jobserver、任务权重、内存预算和 codegen queue。

这四点共同解释了多个表象：

- 冷 `check` 已有 7 秒级耗时和约 490 MiB RSS；
- 70,000 级 query slot/dependency 访问，却只有接近单核的 CPU 利用率；
- query value clone 与 aggregate product 仍扩大内存和回归面；
- provider、type alias、resolver 等 API 容易出现新旧双轨或大量 callback/context glue；
- 64 个 crate 并没有消除巨型实现文件，反而增加依赖扇出和跨边界 DTO；
- 测试已能原样 `cargo test`，但仍依赖侵入各测试入口的全局 permit，在 harness 层补偿单次编译的高 RSS 与内部调度缺失。

Rust 的核心优势不是“查询更多”，而是 `GlobalCtxt/TyCtxt`、arena、统一 interner、稳定 dep-node、生成式 query plumbing 和 codegen unit 构成一个经过性能设计的 compiler kernel。Zig 的核心优势不是“单体文件”，而是 `Compilation/Zcu/InternPool/PerThread` 对状态、紧凑 ID、增量依赖和任务所有权有清楚定义。

Nia 不应回到 Kern 的粗粒度全量流水线，也不应继续在当前基础上增加 query、Arc、小 crate 或环境变量。正确方向是：**先重建会话级身份和产品所有权，再重建查询存储与统一调度，最后才做持久增量与 codegen 并行。**

## 3. 结论分级

| 优先级 | 结论 | 当前后果 | 目标 |
|---|---|---|---|
| P0 | session type identity 与 type-lowering product 已统一，旧 view/migration 实现层已删除 | Phase B acceptance 已完成 | 保持 canonical store 单轨，不恢复 snapshot/view facade |
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
         -> session TypeStore + temporary module views
         -> provider discovery / semantic facts
         -> BodyIr -> FunctionBody -> BackendProgram
    -> driver provider-demand fixed point
    -> reachability fixed point
    -> monomorphization through the session type store
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

健康的 Nia 不应是“没有中心”，而应是：集中语义身份、revision、存储、依赖规则和调度规则；分离 parser、type、trait、const evaluation、optimization 等算法；显式定义算法的输入、输出和所有权。中心内核与算法解耦并不冲突，稳定内核恰恰是外围模块能够真正解耦的前提。

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

### 7.1 Session-wide 类型身份与 semantic product 已完成统一

Phase B 开始前，`InternedTyId` 包含 module interner identity 与 local slot，同一结构类型在不同模块可以有不同 ID，解释 handle 也必须携带对应快照。当前 identity model 已收敛为 `TypeStoreId + TypeStoreIndex`：handle 恰好 8 bytes，不包含 module、origin 或 visibility identity；共享 canonical core 对 `TyKind` 做 session-wide canonicalization，因此不同模块发布的同一 primitive 或 structural type直接得到同一 ID，不同 compiler session 仍由 store identity 隔离。

所有 pass 与测试 fixture 都从唯一 `TypeStore` 解释 handle，并通过 cloneable、write-only `TypeStoreAppend` 发布合成类型。module visibility log、snapshot/checkout、same-shard guard、recursive import、physical origin 与旧 interner identity 类型已经删除。发布结构类型时只验证 referenced child 已存在于同一 store；语义可见性显式来自当前执行模块、definition identity 和 query facts，不再借 storage view 表达。

`TypeLowering` 只包含 source-addressable type facts、const expressions 与 diagnostics；所有入口要求显式 `TypeLoweringContext`。Normalization、layout、trait/body、const、reachability、mono/backend 和 LLVM 均使用同一 canonical read/append 契约，semantic roots 由各产品显式提供，不扫描 storage 发现输入。跨 revision update 保留旧 slot 含义，跨 database handle 被拒绝，相关自动化验证覆盖 Phase B 的 local/stale identity 边界。后续 Phase C 不得为 query ownership 便利恢复 type snapshot 或 view facade。

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

**根部重构原则：架构目标优先于现有调用面的数量。** 当统一 identity/storage 模型与“多数算法仍依赖 module view、snapshot 或 recursive import”的现状冲突时，应判定这些算法的输入契约和实现仍属于旧架构，而不是据此削弱 type store、增加兼容 facade，或把旧读取模型永久化。影响面大只说明架构债务横跨多个域，不构成保留债务的理由；应沿依赖方向重构全部受影响算法，并用编译错误迫使调用点一次性收敛。迁移 adapter 只能位于一个明确、持续前移的边界，不能因调用者多而扩展为双轨 API。该原则同样适用于后续 query storage、provider graph、IR ownership 和 executor 重构。

1. `TyId` 是 compilation-session local typed index，不携带 module owner；primitive/structural type 本来没有源码模块归属，nominal type 通过 `TyKind` 内的 stable definition identity 表达来源。
2. session type store append-only，已发布 slot 在 session 生命周期内 immutable 且永不复用。revision 改变 syntax/def -> type 的 fact，不重编号整个 type store；否则 incremental query 无法保留稳定 handle。
3. 不同 compiler session 由 store identity 隔离，store API 必须拒绝 foreign handle。当前 owned query product 尚不能用 Rust lifetime 静态限制时，以该检查作为 stale-handle 边界；session drop 后 handle 不具备可解释性。
4. 跨进程/跨 session 使用独立 `StableTyKey`：它由 stable definition/source identity 与结构数据 canonicalize，只在 persistence 边界映射为 local `TyId`，不能替代热路径 index。
5. type store 集中 ownership、canonicalization 与同步策略，但算法 crate 只能通过 typed API 查询/intern，不能获得可任意修改其他语义状态的 global world table。
6. 迁移从 type lowering 开始，依次经过 normalize、trait/body、layout、mono/backend；每个域迁移完成时删除该域的 snapshot/import 路径。adapter 只存在于“已迁移前缀 -> 下一个未迁移域”的单一边界，不提供 module-local/session-global 两套公开选择。

这也排除了一个表面简单的方案：只给旧 `TyInternerId(ModuleId)` 叠加 revision generation。declaration/full/signature type lowering 原先会分别从零构建同 module interner，正是依赖相同 module-derived id 和 prefix 假设才可互用；先强化 id 而不先统一 ownership，只会把隐式耦合变成大面积不兼容。当前实现先让 `CompilerContext` 持有唯一 `TypeStore`，四类 type lowering 在同一 store 的 module shard 内串行 canonicalize/append，再返回 snapshot 供未迁移下游使用；这样 store identity 与 append-only 规则已经真实生效，而不是 generation wrapper。规范已同步到 `docs/architecture.md`；统一 index 和 snapshot 删除完成前仍不得把 Phase B 标为完成。

### 7.4 Definition 稳定性

Nia 的 `ModuleId(u32)`、`DefId(u64)`、`SourceId(u32)` 当前主要是装载/收集过程中的编号。源码中没有与 Rust `StableCrateId + DefPathHash` 或 Zig `TrackedInst` 等价的完整跨 revision identity 层。

`ModuleId` 的 session-local 数值也不应继续作为公开 tuple 字段。后续应将其改为 opaque handle，由显式 allocator / `ModuleGraph` 分配；入口模块是 graph role，只能通过 `graph.entry()` 获取，不能定义 `ModuleId::origin()`、`ENTRY = 0` 或测试专用魔法零值。需要模块身份的算法必须接收真实 `ModuleId`；只有语义上确实允许缺失时才使用 `Option<ModuleId>`，compiler-generated/builtin 来源则应使用明确的 origin 枚举。测试通过 `nia-test-support` fixture 创建 graph 或 allocator 并取得 ID；不使用 `#[cfg(test)] ModuleId::test(0)`，因为依赖 crate 看不到该 API，而 feature 会污染统一构建。opaque 迁移还应删除会自行捏造 `ModuleId(0)` 的无 ID lowering convenience API；`index()` 只作为稠密表边界，mangling/persistence 最终应依赖 stable identity，而不是泄漏本地编号。

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

## 9. 语义分析、Trait 与 Const Evaluation

Nia 已经把 type lower、type resolve、normalize、trait solve、body check、const check 等职责拆开，这比 Kern 的粗阶段更利于演进。问题是拆分主要发生在 crate 和数据产品层，公共语义内核仍不够稳定。

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
    -> BodyIr (typed tree, per-module body/static maps)
    -> FunctionBody (CFG blocks, but expressions仍是 nested owned tree)
    -> function optimization
    -> BackendModule/BackendProgram
    -> LLVM IR
```

`BodyIr` 是 `Clone + PartialEq` 的完整模块产品，包含所有 function bodies 和 global init，但其 interner snapshot 已在 Phase B 删除；需要解释其中类型 handle 的消费者显式借用同一 session view。`FunctionBody` 同样是深层 owned 结构，locals/scopes/blocks 用 Vec，但 block 内的 `FunctionExpr` 仍大量使用 `Box`/`Vec` 递归树。

`BackendModule` 再聚合：

- interner；
- const facts；
- 所有 layouts；
- structs/unions/enums；
- globals/global instances；
- functions/function instances；
- vtables；
- generic instantiations。

早期整个 `BackendProgram/BackendModule/BackendFunction` 体系普遍 derive `Clone`，使backend IR同时承担codegen输入与可深拷贝的program snapshot。当前顶层`BackendLowering`、`BackendProgram`和`BackendModule`的深`Clone`已经删除；内部item只在materialization、substitution或优化确有独立owned value时保留局部clone能力。

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

Nia 的 monomorphization 此前会为每个模块 clone `TyInterner` 到 `working_interners_by_module`，然后在多个 map 中跟踪 substitutions、type instantiations、source edges、effective generics、symbols，输出又携带所有 working interners。Phase B 的当前迁移已经删除这条 writable fork：collector 直接借用 session `TypeStore`，输出只保留 instances 与 diagnostics；递归类型检查用短事务读取单个 `TyKind`，投影求解只锁目标 shard，mangling 在不会重入 store 的有界事务中完成。

这套实现功能上已经不简单，但结构上有三个问题：

1. mono identity 仍携带 `arg_module_id + session type IDs`，其中 view/module context 在统一 ID 后需要重新审查并删除冗余部分；
2. collection 不是统一 dependency graph 中的 item traversal，而是先聚合 module inputs；
3. backend 的 body/function 多快照、writable clone、`BackendModule.interner`、调用栈最终 snapshot map 与 recursive view import 已删除；backend 自身读 canonical store、只以 checkout 追加 synthesized type，但其调用的 trait solver/layout 算法仍接受旧 working-interner 契约，需要继续从算法根部迁移。

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

第一轮 `nia-test-support` 虽建立了跨进程 `ResourcePool`，但预算仍过于乐观：每个 compiler unit 按 1.5 GiB、只为系统预留固定 1 GiB，8 GiB WSL 因而允许 4 unit；build test 申请两个 unit，正好可以同时放行两个完整 build 链。2026-07-15 的真实 OOM 证明这不是可靠默认值：两个 `nia` 主编译器各约 1.49 GiB RSS，另有两个 build-script 编译器，而同机 HLS 约 2.9 GiB，最终 7.6 GiB RAM 与 2 GiB swap 全部耗尽并触发 WSL 异常重启。

修订后的外层 integration harness 仍以 1.5 GiB 为 compiler unit、build 为两个 slot/一个 memory unit，但测试总预算最多只占有效内存的一半，并在实际取得跨进程 slot 后检查 system/cgroup 当前可用内存；压力不足时释放 slot 等待，而不是继续启动子进程。容量取 CPU、system memory 与最紧 cgroup 祖先限制的共同约束并最多为 8；cgroup v2 同时按各祖先的 `memory.max - memory.current` 取最小余量，避免租赁机或容器把限制设在父 slice 时误判为无限。Linux 使用 PID + process start time 回收崩溃遗留 slot，无法可靠判活的平台按 age 回收；无法取得内存上限时重编译测试退化为串行。workspace 级 `RUST_TEST_THREADS=2` 已删除，普通 unit test恢复libtest自然并发且不获取全局compiler permit；只有CLI/Driver等声明完整进程/session的integration入口参与外层资源门控。`CompilerDatabase`与LLVM public API在test/non-test构建下语义一致。

这里不应把 WSL、日常物理开发机和租赁开发机建模为三类 profile。正确的配置维度是进程实际可用的 CPU affinity/quota、cgroup/VM 可见内存和用户显式覆盖：WSL 走 Linux 路径并看到其 VM 资源，容器或受限租赁机采用 cgroup 上限，裸 Linux 开发机采用系统资源。Rust 仓库本身主要通过 `x.py`/bootstrap 统一 suite、jobs/jobserver、compiletest 并发和 CI 机器配置，而不是为机器类别维护不同测试语义；Nia 要吸收的是这个“统一调度入口 + 可覆盖资源预算”的分层。根目录无参数 `cargo test` 的外层资源声明是integration harness职责，不是旧compiler API的兼容层，也不能演变成第二套production调度器。

分组实测给出的 `nia-compiler-query` 约 866 MiB、`nia-driver` 约 1.46 GiB、`nia-cli` 约 1.69 GiB 只能说明 1.5 GiB/unit 的数量级，不能证明“系统总内存减 1 GiB”足以覆盖并发峰值。OOM 现场进一步表明 build 链应按复合任务计费，测试预算也必须给开发工具和 WSL 宿主行为留下比例余量。

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

第一轮资源预算后，根目录无参数 `cargo test` 曾在同一 WSL 环境完整通过，但单次成功不足以证明保护可靠；2026-07-15 的后续原样运行在 `nia-cli` 的 `commands` 二进制中触发了上述有内核日志佐证的 OOM。修订预算并删除 workspace 级 libtest 限线程后，`cargo test -p nia-cli --test commands` 已在自然并发下 50/50 通过，完整 build case 在 8 GiB 环境按权重串行进入；随后根目录原样 `cargo test` 也完整通过，覆盖 CLI commands/native execute、176 个 LLVM/codegen、107 个 compiler-query、484 个 driver case 与全部 doc-test。运行后本 boot 无新增 kernel OOM 记录，2 GiB swap 仅使用 268 KiB；严格 workspace/all-targets/all-features Clippy 同样无 warning。这个结果恢复了默认入口，但仍只证明当前保护在本机 workload 下成立，后续应以重复运行和不同 cgroup 预算继续验证，而不能再由单次成功推导永久可靠。

Nia 约有 1,600 级 `#[test]`，覆盖面可观，这是资产。主要问题是：

- 大量 compiler integration case 嵌在生产 crate 的 `src/tests` 或巨型 query 文件中；
- 每个 case 常创建临时工程并完整跑 compiler；
- libtest 默认并发与 compiler 内部并发相乘；
- 共享 `/tmp`、进程 ID/计数器命名、linker/环境变量容易形成隐含全局状态；
- `nia-test-support::compiler_permit` 曾侵入 compiler 公开入口，现已删除；
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

现已增加 `tools/perf.py` 与固定 `benchmarks/`/examples workload。runner 先用显式 `perf-alloc` feature 构建一次 instrumented release compiler，再让 minimal、strings/slices、ArrayList、traits、const evaluation 和完整 emit-exe 各自在新进程运行，并汇总 schema-versioned JSON。普通 compiler build 不安装 counting allocator，日常编译与测试的 allocation hot path 没有额外原子检查；自定义或 `--no-build` compiler 缺少 instrumentation 时 runner 会直接拒绝，而不是记录伪造的零值。compiler 的同一 `nia-timing` collector 直接输出 wall/user/sys、max RSS、CPU utilization、stage/query timings 及 typed counters；driver 上报 query executions/hits、provider-demand rounds、checked/reachable bodies，LLVM 上报 unit/object-reuse，instrumented detail timing 还能汇总实际 query value clone count/bytes。这里没有通过脚本解析人类 timing 文本，也没有新增隐藏环境变量。

2026-07-14 在当前 WSL release build 上的一次 smoke baseline（单次样本，仅验证量级与管线，不作为跨机器阈值）：

| workload | wall | max RSS | query executions | checked/reachable bodies |
|---|---:|---:|---:|---:|
| minimal check | 0.003 s | 39 MiB | 76 | 1 / 1 |
| strings/slices check | 1.535 s | 323 MiB | 4,082 | 94 / 94 |
| ArrayList check | 2.370 s | 480 MiB | 5,554 | 144 / 144 |
| traits check | 1.460 s | 318 MiB | 3,920 | 85 / 85 |
| const-eval check | 0.071 s | 42 MiB | 176 | 1 / 1 |
| ArrayList emit-exe | 3.699 s | 1,042 MiB | 6,239 | 152 / 183 |

emit-exe 生成 30 个 LLVM units，当前 object reuse 为 0，直接暴露了阶段 G 的缺口。runner 在 machine metadata 中记录 CPU affinity、cgroup CPU quota、system/cgroup/effective memory；它不建立 WSL、物理工作站和租赁机 profile。首个样本只建立了 value clone count，下面的第二个 instrumentation 切片继续补齐 allocation/clone bytes。

第二个 instrumentation 切片把成功 alloc/dealloc/realloc 次数和 requested bytes 接入同一个 detail collector，并按线程测量 owned query value clone 期间的真实 allocator traffic；它由 perf runner 的显式 `perf-alloc` build 启用，不改变普通 compiler binary 的 allocator。collector flush/JSON 序列化发生在计数停止之后。新的 release smoke 样本为：

| workload | Rust allocated bytes | query value clones | query clone bytes |
|---|---:|---:|---:|
| minimal check | 1.24 MiB | 126 | 0.16 MiB |
| strings/slices check | 1.31 GiB | 32,193 | 50.3 MiB |
| ArrayList check | 2.12 GiB | 49,947 | 99.1 MiB |
| traits check | 1.24 GiB | 31,242 | 46.8 MiB |
| const-eval check | 364 MiB | 340 | 1.25 MiB |
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
|   +-- temporary module-view snapshot/import traversal
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

### 19.1 初始审查时已确认的核心缺陷

以下是 2026-07-14 的审查基线；已完成和仍在迁移的项目以第 21 节各阶段进展为准，不能把本清单直接当作当前工作树状态。

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

### 21.1 Const 语义边界

Nia 的公开语言不再使用 Zig 式 `comptime` 总括编译期已知性、求值、结构选择和局部状态。`const` 只表示不可变、无运行时存储、无独立地址身份的编译期值；条件源码选择继续由 `@[if ...]` 独立承担。模块、关联和局部 const 声明都不存在 `const mut`。常量求值算法仍可在 `const fn` 或 const initializer block 内使用瞬态 `let mut`，但这些局部只能属于一次求值 call frame，不可修改全局 const、不可形成可观察的求值顺序，也不可让可变引用或临时地址逃逸到最终结果。这样 const query 对外仍是由函数、参数、target facts 与显式追踪输入决定的纯结果。

实现必须与这个边界同构：AST binding kind 使用 `Const` 与 `Let/Static { is_mutable }` 的枚举，而不是 `is_const × is_mutable` 布尔组合；const evaluator 的 assignment target 只允许本次调用的 mutable local。内部 crate/API 统一采用 `nia-const-ir`、`nia-const-eval`、`nia-const-check` 和 `Const*` 命名，旧 `comptime` 关键字、crate、type alias 与兼容入口全部删除。`const fn` 是否在 runtime-representable 参数/返回类型下也可运行时调用，是下一层独立语义与 IR 统一任务，不能作为关键字迁移的隐式副作用。

## 22. 分阶段路线图

### 阶段 A：建立基线与防回归指标

在架构改动前固定 4–6 个 workload：minimal、slices/string、ArrayList、trait-heavy、const-eval-heavy、完整 emit-exe。记录：

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

执行约束：不能用“多数算法当前依赖旧 view”限制重构范围。若 canonical store 的自然 API 暴露出 trait solver、normalization、body、layout、reachability 或 backend 的输入契约不成立，就重构这些算法及其调用链；不能在 store 侧复制旧 module-local 语义来迁就调用者。完成度以旧数据流实际删除为准，不以新 API 已可选使用为准。

1. 定义 session-local typed index、跨 revision stable key、generation/reuse 规则及 stale-handle 不变量。
2. 引入 session-owned type store 与统一 `TyId`。
3. 为现有 module-local interner 建一次性迁移 adapter。
4. 逐域迁移 type lower -> normalize -> trait/body -> layout -> mono/backend。
5. 每迁移一域就删除对应 snapshot/import API。
6. 删除 working interner copies 和跨 interner recursive import；`ProgramFunctionBodyInterners` 已在上游快照边界迁移时删除。

Acceptance：生产代码中不再出现跨 interner type import；backend product 不再内嵌 interner snapshot；同一类型 handle 可在一个 compilation session 的 module/stage 间直接比较；stale/local/stable identity 规则有自动化验证。

进展（2026-07-14）：identity/ownership contract、type lowering 与 normalization 生产路径的迁移切片已完成。`CompilerContext` 现在持有 compilation-owned `TypeStore`；declaration/full/signature/signature-comptime type lowering 与四类 normalization provider 全部在同一 append-only module shard 中 intern；`TyInternerId::for_module` 已删除，store identity 会拒绝不同 compiler session 的 foreign handle。Normalization 不再先 clone interner 再私有写入，也没有保留旧 API：唯一算法入口显式接收 mutable interner 与本次 lowering 的 input IDs，query provider 只负责提供 store transaction，因此 signature subset 不会误归一化 store 中其他 subset 的类型。自动化测试覆盖跨 update prefix/旧 slot 不变、跨 database 隔离、normalization 只追加且只处理显式输入，以及 signature/full 两种执行顺序共享 alias expansion ID。后端实例布局原先重新归一化后仍读取旧 interner，并分别维护 local/foreign 两条重复路径；现在它对既有 backend working interner 使用同一算法、统一读取归一化 snapshot，并合并为一条实例布局路径。trait/body 迁移的第一切片也已开始：`TraitSolverContext::solver*` 原先为 enum 分类 clone mutable working interner，导致 solver 后续追加的本地 nominal type 对分类视图不可见；现在 `TraitSolver` 直接持有 enum 元数据并从同一个 working interner 读取类型，删除这份冻结 snapshot，回归测试覆盖 solver 创建后追加的本地 enum handle。原样 `cargo test` 与严格 workspace Clippy 均通过；相对 normalization baseline 的六 workload perf guard 通过，query executions 全部不变，ArrayList / strings+slices / traits / emit-exe allocated bytes 分别下降 1.31% / 1.22% / 0.97% / 1.48%；单样本 wall 波动不作为性能收益声明。过渡期 shard identity 紧凑编码为 64-bit，`InternedTyId` 为 16 bytes；该尺寸暴露的 `FunctionForHeader` 大小悬殊已通过只对 recursive condition 增加 ownership indirection 修正，没有 Clippy allow。Phase B 仍在进行中：当前 handle 仍包含临时 module shard，`TypeLowering`/`TypeNormalization` 仍为未迁移下游携带 snapshot，body/comptime/layout/mono/backend 的 working interner 与跨 interner import 也仍存在。下一切片继续迁移 body/comptime 的 working interner ownership，并将同 module append 与真正跨模块 import 分开；不能用 type alias 或永久 adapter 假装统一 `TyId` 已完成。

进展（2026-07-15）：comptime 的本模块 working interner 已从隐式 clone/replace 改为显式阶段所有权链：array lengths -> enum values -> values -> typed facts -> final check 每一步直接接管上一阶段 interner，并断言它是初始 normalization input 的 append-only extension；`finish_local_interner` 丢失 shard 时不再 fallback clone，而是 ICE。`import_ty_into_module*` 也统一为先验证 target shard 已含 handle 并直接复用，只有 handle 缺失时才获取 source snapshot 做 import；同时删除了 `active_interner_for_type` 返回 owned snapshot 后的二次 clone。comptime fixture 统一验证最终 interner 与 lowering identity 相同且保持 prefix。原样 `cargo test`、严格 workspace Clippy 和六 workload perf guard 均通过；相对 trait-working baseline，query executions 全部不变，comptime RSS -1.18% / allocated -0.015%，emit-exe RSS -0.044% / allocated -0.107%，其余波动均在 guard 内，单样本 wall 不作为性能结论。Phase B 仍未完成：第一阶段仍需从 normalization snapshot 建立 working interner，各 comptime query product 仍内嵌阶段 snapshot，外模块 signature import 也仍存在；下一步应把 query provider 的本模块 comptime mutation 接入 session `TypeStore` transaction，再删除这些阶段产品中的 interner 字段。

进展（2026-07-15）：语言与实现的 compile-time value 模型已单轨收敛为 const。lexer/parser/AST、标准库、测试和规范不再接受或生成旧 `comptime` 关键字；`const mut` 从 AST 状态空间删除，局部与 item binding 分别由 `LocalBindingKind::{Let, Const}` 和 `ItemBindingKind::{Static, Const}` 表达，普通 mutable/immutable local 与 const local 也拥有不同的 semantic kind。有效 mutation 用例统一改为 `const fn` 内的 `let mut`，const assignment 仍只写本次 call frame。`nia-comptime-ir/engine/check` 及全部 `Comptime*` API 已原子迁移为 `nia-const-ir/eval/check` 与 `Const*`，没有保留 crate alias、type alias、`const let` 或旧 parser 分支；旧关键字拒绝、顶层/局部/关联 `const mut` 拒绝以及 176 个 const-eval driver 回归均有验证。原样 `cargo test`、`cargo check --workspace` 与严格 workspace Clippy 均通过。该语义收敛不改变 Phase B 的剩余所有权问题：const query products 仍携带阶段 interner snapshot，下一步仍是接入 session `TypeStore` transaction 并删除产品内 interner 字段。

进展（2026-07-15）：const 的本模块类型 mutation 已接入 compilation-owned `TypeStore` transaction。`ConstArrayLengths`、`ConstEnumValues`、`ConstValues`、`ConstTypedFacts` 不再内嵌或逐阶段转交 `TyInterner`；Analyzer 直接借用 session module shard，`finish_local_interner` 与 phase snapshot handoff 已删除。外模块 working snapshot 暂时保留在 Analyzer 内部，但会与 session 中更新后的 source snapshot 按 append-only prefix 选择较新视图，只有真正分叉才 ICE。transaction 不能跨同模块 provider callback 盲目持锁：local trait/extension facts 现在以 shared query handle 预取、仅在实际请求时展开，并有 range const 实际追加 store slot 的阶段回归以及 177 个 const-eval driver 测试覆盖 imported const fn/array length 路径。原样 `cargo test`、严格 workspace Clippy 与相对 `628a42dc` 的同机六 workload perf guard 均通过；const-eval query executions 不变、allocated bytes -0.058%、RSS -0.59%，其他大型 workload allocated bytes 在 -0.24% 到 +0.41%、RSS 在 +0.34% 到 +0.72% 内，新增 shared-fact dependency 使 strings/ArrayList/traits query executions 分别增加 8/6/9（均低于 0.24%），单样本 wall 不作为收益结论。`ConstCheck` 仍保留一份 snapshot，明确作为未迁移 body/backend 的临时边界；下一切片应把 body working interner 接入同一 store，再删除这最后一份 const snapshot，而不是把它转移到另一个 DTO。

进展（2026-07-15）：body 的本模块类型 mutation 也已接入同一 compilation-owned `TypeStore` shard，const -> body 的 snapshot 边界已删除。`BodyConst` 与 `ConstCheck` 均不再携带 `TyInterner`；body checker 的 production working interner 是 store transaction 的 mutable borrow，typed const query 也直接向该 borrow 追加类型。只有 `clone_for_type_compare` 的推测性匹配使用明确的隔离 snapshot，`BodyIr.interner` 则暂时保留为尚未迁移 layout/mono/backend 的唯一边界快照；prechecked body 和 seed 只允许是 session shard 的 append-only prefix，不能再替换 store。迁移首次运行暴露出本模块 function-signature resolver 在持锁期间重入 type lowering 的真实死锁：provider 现在在 transaction 前获取 shared function-subset/interner handle，具体 signature 仍按请求逐项构造并缓存，因此保留 precise/lazy query 依赖而不复制完整 map；`TypeStore` 同时增加同线程同 shard 重入守卫，使类似架构错误立即 ICE 而不是永久挂死 WSL。自动化回归验证 body 新建的 `[3]i32` 类型实际追加到 session shard、`BodyIr` 等于最终边界快照，以及重入会快速失败。无参数、无环境变量的原样 `cargo test` 完整通过，严格 workspace/all-targets/all-features Clippy 以 `-D warnings` 通过。相对 `628a42dc` 的同机六 workload guard 通过；optimized lazy resolver 后 ArrayList / strings / traits / const-eval / emit-exe allocated bytes 分别为 +0.010% / -0.0001% / +0.159% / -0.173% / -0.235%，RSS 全部在 0% 到 +0.72%，query execution 增量与上一 const-store 切片相同（strings +8、ArrayList +6、traits +9，均低于 0.24%），单样本 wall 不作为性能结论。Phase B 下一边界是让 layout/monomorphization/backend 消费 session-owned 类型并删除 `BodyIr.interner`；不能把该快照搬入新的 aggregate product，也不能把 speculative snapshot 扩大成 production 双轨 API。

进展（2026-07-15）：function IR lowering 的本模块类型写入已从 `BodyIr.interner.clone()` 迁到 session `TypeStore` transaction。`LoweredFunctionBody`、`nia_function_lower::LoweredFunctionBodies` 与 compiler-query 的同名 batch product 均不再携带 `TyInterner`；for-in lowering 合成的 optional item type直接追加到调用方 shard，并有 prefix/slot-growth 回归。codegen provider 在所有 function body lowering 完成后才为尚未迁移的 monomorphization/backend 获取短生命周期 module snapshot map，该 map 只存在于一次 provider 调用栈，不进入 query value、checked IR 或 function-lowering product。49 个 function-lower、107 个 compiler-query、98 个 backend-lower、176 个 codegen 与 484 个 driver 测试通过，严格 workspace Clippy 无 warning。相对 body-store candidate 的六 workload perf guard 通过，query executions 全部不变；allocated bytes 在 -0.005% 到 +0.012%，RSS 在 -1.17% 到 0%，单样本 wall 不作为性能结论。下一切片应迁移 `MonoCollector.working_interners_by_module` 与 `Monomorphization.type_interners`，随后让 reachability/backend/codegen 从 session 类型访问读取并删除 `BodyIr.interner` 与 `ProgramFunctionBodyInterners`；provider 临时 snapshot map 不能固化成新的所有权层。

进展（2026-07-15）：monomorphization 的 writable type ownership 已迁入 session `TypeStore`。`MonoCollector.working_interners_by_module`、只读 `interners_by_module` 与 `Monomorphization.type_interners` 已删除；唯一 collector 入口显式接收 store，先验证输入 snapshot 是对应 shard 的 prefix，再让实例化类型直接追加到 shard。递归 generic/depth/import 检查不跨递归持锁，trait projection 只在 solver 调用期间锁目标 shard，mangling callback 只读预取 map；回归测试证明 nested generic body 实际向 session 追加 `&i32` 类型。backend 不再把 mono result 当 type store，而是在 function lowering 与 mono 完成后读取 compiler-query 提供的最新短生命周期 session snapshots；backend fixture 也改为完整 store pipeline，没有测试专用兼容 API。原样根目录 `cargo test`、严格 workspace/all-targets/all-features Clippy 均通过；相对 function-store baseline 的六 workload perf guard 中 query executions 全部不变，allocated bytes 在 -0.052% 到 +0.002%、RSS 在 -1.92% 到 +0.16% 内，单样本 wall 不作为性能结论。Phase B 尚未完成：`BodyIr.interner`、`ProgramFunctionBodyInterners` 与 backend snapshot/import 路径仍存在，下一切片应把 layout/backend 的读路径接到 session type access 并删除这些边界，而不是把 snapshot map 固化成新产品。

进展（2026-07-15）：layout 的 writable type ownership 已迁入 session `TypeStore`。`LayoutComputer` 与唯一公开计算 API 直接复借调用方 mutable interner，`Layouts.interner` 已删除；production module/signature/executable providers 在对应 shard transaction 内完成 root 收集、跨模块 signature import、generic substitution 与 layout 计算，standalone/body/backend 则对各自唯一 working interner 使用同一 API，没有保留 read-only/owned 双轨。backend instance layout 重新 normalization 后也继续写同一 working interner，trait solver 的 layout equivalence 不再读取 layout product 私有快照。跨模块 generic signature 回归证明 layout 新建的 array type实际追加到 session shard。迁移首次运行暴露出 executable layout 在持锁期间 lazy 求 array length 导致同 shard 重入；provider 现在 transaction 前预取本模块 facts，外模块 array length 独立使用 signature facts。const program context 也显式统一 module/normalization/signatures 的 full-or-signature policy，因此 type-only module 保留 array length且不会升级执行 full type/value resolution；`@size/@offset` 路径同时删除了临时整份 defs/signatures clone。原样根目录 `cargo test` 完整通过，严格 workspace/all-targets/all-features Clippy 无 warning；compiler-query 108、backend-lower 98、body-check 139 个 crate 测试均通过。全量验收还发现 test resource request 把 build 的 2-slot 独占权重误当成两个并发 compiler memory unit，在 7.6 GiB WSL 上会无进展等待；调度 request 现将 slots 与 memory units 正交表达，build 仍独占 2 slots但按一个串行 compiler workload门控，回归测试覆盖该不变量，无参数 `cargo test` 不再需要外部并发限制。Phase B 仍未完成：`BodyIr.interner`、`ProgramFunctionBodyInterners` 与 backend snapshot/import 路径仍是下一切片，不能把 layout 已删除的 ownership 重新包装进 backend aggregate。

进展（2026-07-16）：上游 body/backend 多快照边界已删除。`BodyIr.interner` 与 `ProgramFunctionBodyInterners` 从生产 API 和测试 fixture 中完全移除；body checker 只产出 typed body/static data，executable facts、incremental seed、reachability 与 backend input 都显式借用当前 session type view。compiler-query 只在 executable fixed-point 单轮或 backend lowering 调用栈内取得短生命周期 snapshot：backend map 在 function lowering、layout 与 monomorphization 完成后统一构建一次，不进入 query value、`BodyIr` 或新的 aggregate product。`BackendTypeContext` 直接从当前 module 的最终 view 建 working interner，不再比较、选择或合并 body/function snapshot；foreign module 也统一查同一 `program_type_interners` view。进一步审计发现 `BackendLowerShared` 原先会对每个 module 重扫整张 program map，并在相同 interner ID 命中时再次深 clone 较新 snapshot，形成模块数平方级的复制流量；shared index 现只按 ID 借用唯一调用栈 view，不再拥有第二份 interner map。旧字段/API 全仓搜索为零，`cargo check --workspace --all-targets`、body-check 139、backend-lower 96、compiler-query 108 个定向测试、严格 workspace/all-targets/all-features Clippy 与无参数无环境变量的原样 `cargo test` 均通过，WSL 未发生 OOM。Phase B 仍未完成：backend 仍 clone writable working interner、执行跨 interner recursive import，并把 `TyInterner` 内嵌进 `BackendModule` 供 codegen 解释 handle；下一切片必须把 backend lowering、backend IR 与 codegen 的类型访问作为一个完整所有权边界共同迁移，不能只删字段后重新引入旁路 lookup 或永久 snapshot facade。

进展（2026-07-16）：backend lowering、Backend IR 与 LLVM codegen 的产品所有权边界已共同迁移。`BackendTypeContext` 不再 clone 当前 working interner，而是在完整 whole-program lowering fixed point 期间独占 checkout 对应 session shard，所有新实例类型直接追加回 `TypeStore`；`BackendLowerModuleInput.type_interner` 与 `BackendModule.interner` 已删除，Backend IR 只保存 typed handle 和 backend facts。`CodegenProgram` 只持有轻量 `Arc<TypeStore>` session handle，LLVM 的单轨 API 显式接收 store，在完整 validation/emission 调用期 checkout program shards，既不恢复 snapshot aggregate，也不对递归类型读取逐次短锁。checkout 被约束为 thread-bound `!Send`，同 shard 重入立即 ICE，Drop 后归还包含新增 slot 的原 shard；自动化回归覆盖追加、重入失败和归还恢复。测试 fixture 同样显式持有 store，没有产品/测试双轨 API。`cargo check --workspace --all-targets`、nia-ty 14、backend-lower 96、codegen 176、compiler-query 108 个定向测试、严格 workspace/all-targets/all-features Clippy，以及无环境变量、无参数的原样 `cargo test` 全部通过；CLI build 测试在资源门控下持续前进并完成，WSL 未发生 OOM 或退出。Phase B 仍不能关闭：`InternedTyId` 仍带 temporary module shard，compiler-query 仍为 backend 调用栈构建最终 module snapshot map，backend 的 foreign type interpretation、`FunctionInstanceRef.arg_interner`/`GlobalInstanceRef.arg_interner` 和 trait candidate 临时 view 仍依赖跨 interner recursive import。下一切片应建立真正 session-wide typed index，并按 type lower -> signatures/trait/body -> reachability/backend 的依赖方向删除全仓 import 与 paired interner 数据；不能在 backend 内再造半全局 ID 或把当前 checkout adapter 宣布为最终 `TyId`。

进展（2026-07-16）：session-wide `TyId` 的前置 identity contract 已收紧：`InternedTyId::owner()` 从 `nia-ids` 删除，纯 handle 不再自行解释语义 owner。monomorphization 统一查询 `TypeStore`，const/body/reachability 查询当前 type view，backend validation/codegen 查询 `ProgramIndex`；没有保留 direct-owner fallback。当前 storage 实现仍从旧 `TyInternerId` 物理布局恢复 temporary module owner，但这一细节只存在于 `nia-ty`，后续改成 global index + owner metadata 不再需要穿透各编译阶段。回归覆盖同 session 的任意 module view 可解析 foreign module handle、不同 session handle 无 owner；全仓 `.owner()` 搜索为零。`cargo check --workspace --all-targets`、nia-ty 15、backend-lower 96、body-check 139、codegen 176、monomorphize 9、const-check 15 与 driver 484 个定向测试均通过，严格 workspace/all-targets/all-features Clippy 无 warning。下一切片应实现 session-wide slot/owner table 与 canonicalization contract；只有 handle 不再含 module shard 且跨模块同一类型直接复用同一 ID 后，才能开始按域删除 recursive import。

进展（2026-07-16）：session-wide canonical type identity 已真正落地。`InternedTyId` 从 `TyInternerId + local slot` 收敛为 `TypeStoreId + global slot`，尺寸由 16 bytes 降为一个 64-bit word；共享 `TypeStoreCore` 负责全 session 的 `TyKind -> TyId` canonicalization 和 append-only slot，module `TyInterner` 只保存 `(TyId, TyKind)` visibility log 与 prefix/snapshot 迁移语义。跨模块 primitive、structural type 得到同一 ID；同 session `import_type_into` 只让 target view 看见现有 type graph并保持 ID，不同 session handle 继续被拒绝，且 interning 会拒绝引用当前 view 不可见或 foreign-session 的子类型。旧 `TypeOwner` 已改名为物理含义明确的 `TypeOrigin`，reachability 不再把它当 module dependency；backend/codegen 完全忽略 origin，从实际包含 handle 的 active views 中按最小 `ModuleId` 确定性选择，避免并发 first-insert 顺序影响输出。core lock 只覆盖单次 canonical lookup/insert，不跨 recursive import 或编译算法。handle 缩小暴露的 `AssociatedConstResolution` large-enum-variant 由仅装箱大 payload 修正，没有 Clippy allow；function lowering 伪造固定 slot `15` 作为 `bool` 的旧布局依赖也已删除，for-in typed IR 显式携带真实 canonical bool handle。`cargo check --workspace --all-targets`、nia-ty 18、function-lower 49、backend-lower 96、body-check 139、codegen 177、compiler-query 108、driver 484 个定向测试、严格 workspace/all-targets/all-features Clippy，以及无环境变量、无线程参数的原样 `cargo test` 全部通过，WSL 未发生 OOM 或退出。Phase B 仍未关闭：下一切片应让 canonical store 直接提供 kind lookup，随后按依赖方向删除 module snapshot/view adapter、identity-preserving recursive import、`arg_interner` paired data 和临时 trait/comparison views，最终删除非语义 `TypeOrigin`；不能因 ID 已统一就保留旧读取体系。

进展（2026-07-16）：canonical store 已成为迁移完成域唯一的 `TyId -> TyKind` 读取源。`TypeStore` 的 append-only canonical core 现在同时保有稀疏四级 `OnceLock` kind arena；直接 lookup 会验证 store identity，通过 `u32` slot 的四个字节定位 immutable cell，并返回绑定于 store 生命周期的 kind borrow，不获取 canonicalization mutex，也不使用 unsafe；不同 session handle 不能被解释。mangling 的全部公开与递归 API 已原子迁移为只接收 `TypeStore`，没有 interner overload；monomorphization 与 backend symbol generation 不再为 mangling checkout module view。LLVM 的 `ProgramIndex` 直接借用 store，validation、compiler-builtin 扫描、layout/ABI/type lowering、static initializer 与 type mangling 均删除 module checkout、owner-interner discovery 和 view-existence 验证；backend module map 只索引 item/layout facts，不再承担类型可解释性。审计同时确认 codegen 的 semantic-equivalence fallback 仍用于 const expression 求值相等，而不是跨 interner identity 补偿，文档已据此纠正。nia-ty 19、backend-lower 96、codegen 177、mangle 2 与 monomorphize 9 个定向测试通过；`cargo check --workspace --all-targets`、严格 workspace/all-targets/all-features Clippy 和无环境变量、无线程参数的原样 `cargo test` 全部通过，WSL 未发生 OOM 或退出。Phase B 仍未关闭：compiler-query/backend 输入、trait/body/reachability 仍存在 module snapshot、recursive import、paired `arg_interner` 与 speculative view；下一切片必须重构这些算法契约并删除旧数据流，不能以调用者数量为由给 store 增加第二套 view API。

进展（2026-07-16）：backend 的旧 type-view 数据流已完整删除。compiler-query 不再为 backend 构造最终 module snapshot map，也不再 clone 携带 interner 的整份 `TypeNormalization`，只借用 `TyId -> normalized TyId` 事实；`BackendLowerModuleInput.program_type_interners`、`extension_interner`、shared input-view index、candidate interner cache 以及 function/global worklist 的 `arg_interner` 全部删除。`BackendTypeContext` 与 generic/error/projection/aggregate scans 统一从 `TypeStore` 读取；foreign refs 只传稳定 `TyId`；extension/trait source 改为 `ModuleId + TyId`，normalization 由显式 module 选择。第一次原样全测在 fixed-buffer allocator 用例暴露出更根部的旧不变量：`TyInterner::intern_local` 仍要求 referenced child 出现在当前 module visibility log，导致 backend 合成跨模块组合类型时 ICE。该约束已改为验证 child 是否存在于同一 canonical core；同 session、view 外 handle 现在可直接组成新类型，foreign-session 或未发布 handle 仍被拒绝，并有正反回归。这意味着 module log 不再承诺类型图传递闭包，依赖该闭包的算法必须迁移，不能迫使 store 恢复 recursive import。nia-ty 20、backend-lower 96、compiler-query 108 和原失败 CLI 用例通过；`cargo check --workspace --all-targets`、严格 workspace/all-targets/all-features Clippy，以及无环境变量、无线程参数的原样 `cargo test` 全部通过，WSL 未退出。Phase B 仍未关闭：reachability 仍携带 `arg_interner` 并递归 import，const/body/program-signature 仍有 snapshot/view 读取，trait solver 与 layout 仍把 mutable interner 同时当 append capability 和读取源。下一切片必须重构这些算法输入为 canonical read + explicit append，随后删除 import API、module visibility log 与 `TypeOrigin`；不能新增 store fallback、overload 或兼容 facade。

进展（2026-07-16）：reachability 的旧 type-view 契约已完整删除。`ReachableModuleInput` 只借用 canonical `TypeStore`，不再携带 `TyInterner`、`TypeLowering` 或 `TypeNormalization` snapshot；compiler-query 的 executable fixed point 也不再为 fact modules 构造 snapshot map。generic substitution、supertrait expansion、extension matching、trait method/vtable 去重与 owner traversal 全部直接解释 stable `TyId`，`arg_interner`、interner-aware seen key、module-view fallback 和 recursive import 均已删除。需要合成结构类型的路径通过独立 `TypeStoreAppend` capability 追加 canonical slot，读取仍只能经过 store；append 不写 module visibility log，referenced-handle 合法性由 canonical core 统一验证。trait method/vtable key 显式包含 use `ModuleId`，保留真正的可见性上下文而不再用 interner identity 代替。nia-ty append 回归证明新 slot 可从 store 读取但不会出现在旧 module snapshot；compiler-query 108 个测试、三个跨模块 generic/where-bound/const-generic supertrait driver 回归、`cargo check -p nia-executable-facts -p nia-executable-reachability -p nia-compiler-query --all-targets`、严格 workspace/all-targets/all-features Clippy，以及无环境变量、无线程参数的原样 `cargo test` 全部通过；CLI commands 50 个重型测试在项目资源门控下持续前进并以 278.41 秒完成，WSL 未退出。Phase B 仍未关闭：const/body/program-signature 与 trait/layout 读取路径仍残留 module snapshot 或 mutable-interner 契约；下一切片继续按依赖方向把 canonical read 与 append capability 分离，不能因为调用者多就保留 view API，最终必须删除 recursive import、module visibility log 与非语义 `TypeOrigin`。

进展（2026-07-16）：program signature、trait solver、body、visible extension 与 layout 的旧 type-view 契约已共同收口。所有 `Program*Signature`、`ProgramTraitImplSignature`、`UserAssociatedConst` 与 `VisibleExtensionsForModule` 不再内嵌或配对 `TyInterner`；compiler-query 不再构造 `signature_type_interner`，消费者直接保留 canonical handles。trait solver 显式借用 `TypeStore` 读取，working interner 仅作合成类型追加目标。body 引入单一 `BodyTypeCx`，全部既有算法的 `get` 固定读取 store，local/program signature import、跨 normalization fallback、working-view adoption 与相关 cache 已删除；alias normalization 直接返回 canonical normalization fact。layout root traversal、generic substitution 与 layout computer 统一为 `LayoutComputationInput + LayoutTypeCx`，读取只走 store、派生类型只走 `TypeStoreAppend`，因此外部 signature 不再增长 module view；Clippy 暴露的八参数旧入口被输入对象彻底替代，没有 allow 或 overload。首次 CLI 回归从 body missing-handle ICE 推进到 ABI layout / iterator resolution 缺失，统一 layout 读取后 build runner 与 freestanding startup 均恢复。body-check 139、compiler-query 108、layout 9、program-signatures 1 个定向测试、严格 workspace/all-targets/all-features Clippy，以及无环境变量、无线程参数的原样 `cargo test` 全部通过；CLI commands 50 个重型测试约 244 秒完成，WSL 未 OOM 或退出。Phase B 仍未关闭：const analyzer、program-signature analysis/normalization helper 与少数 frontend comparison path 仍读取 module views，`try_import_type_into`、module visibility log 与非语义 `TypeOrigin` 尚未删除；下一切片继续迁移这些根部算法，不能恢复 store fallback 或 signature interner。

进展（2026-07-16）：const analyzer、program-signature analysis 与 compiler-query semantic-use 收集器的剩余 type-view 读取已收口。const 现在以 `ConstTypeCx` 固定从 canonical `TypeStore` 读取，module-local/foreign `ConstAppendView` 只满足 trait/layout 的显式 append 契约；generic substitution、primitive 获取、type origin 查询与 typed const value 复用都不再查询、选择或递归复制 snapshot。`ConstProgramContext.value_type_normalizations` 与跨 crate `import_const_value_type` 已删除，body 直接验证并复用 canonical runtime handle。program-signature 的 module/visibility 输入显式借用 store，type equivalence、extend target/trait 解构、alias visibility、builtin overlap、supertrait/associated-type substitution 全部使用 canonical read + `TypeStoreAppend`；Clippy 暴露的仅递归传递 interner 参数链被从整个 goal expansion API 删除，没有 allow。compiler-query 的 associated-const projection 与 array-length dependency scan 也不再读取 `TypeLowering.interner`。最后一个外部 import 测试改为 canonical handle 相等后，`nia-ty` 的 `import_type_into`、`try_import_type_into`、`TypeImportError` 及 recursive graph adoption 实现被整体删除，全仓旧 API 搜索为零。严格 workspace/all-targets/all-features Clippy 无 warning；无环境变量、无线程参数的原样 `cargo test` 以退出码 0 完整通过，约 5 分 26 秒，CLI commands 与 doc tests 均完成，WSL 无 OOM/退出。Phase B 下一切片转向真正的根：让 normalization/type-lowering 与 layout root enumeration 不再依赖 module visibility log，随后删除 snapshot/checkout migration API、`TypeOrigin`、`TyInternerId` 和 view 层；不能把已删除的 recursive import 换成 store fallback 或新的 facade。

进展（2026-07-16）：normalization 产品的最后一个 type view 已删除。`TypeNormalization` 现在只包含 `TyId -> normalized TyId` facts 与 diagnostics；唯一 `normalize_module_types(TypeNormalizationInput)` 入口显式区分 canonical `TypeStore` 读取、mutable append target 和本次 lowering 的 input roots，算法内部不再通过 interner 解释任何 handle，也没有旧参数入口或 compatibility facade。所有 production、standalone 与测试调用点已原子迁移；query/const/body/layout 的 prefix assertion 改为比较真实 `TypeLowering` 与 session shard，layout-root 和 comparison 等尚未迁移的 append 场景直接从 store 取得短生命周期 module view，不再借 normalization DTO 携带 snapshot。进一步沿根部删除了 `ConstInput.interner` 与 `TypedConstQueryInput.base_interner`：const 的读取固定走 store，local prefix 基线来自 lowering，primitive fallback 走 `TypeStoreAppend`，foreign append context 从 store 临时取得；为构造 normalization snapshot 而存在的 test/helper locals 与参数链也全部删除。normalization 测试使用同一个 `TypeStore` fixture，并只从 store 断言 alias expansion、layout builtin 和显式 root 行为；compiler-query 回归改为验证 normalized handles 已发布到 canonical/session store，而不再验证 DTO interner identity。8 个受影响 crate 的 364 项定向测试、`cargo check --workspace --all-targets`、严格 workspace/all-targets/all-features Clippy，以及无环境变量、无线程参数的原样 `cargo test` 全部通过；CLI commands 50 项自然并发约 239.92 秒完成，WSL 未 OOM 或退出。Phase B 现约完成 85%，下一切片只应继续消除 `TypeLowering.interner`、layout full-scan/root enumeration 和 temporary append-view ownership，最终删除 module visibility log、snapshot/checkout migration API、`TypeOrigin` 与 `TyInternerId`；不能把纯 normalization facts 再扩展成 storage/view 产品。

进展（2026-07-16）：normalization 与 layout 的输入发现已从 module visibility log 分离。`TypeLowering::explicit_type_roots` 从 source-addressable type-use facts 生成确定序、去重的 roots，所有 frontend/standalone normalization 调用只处理这些 roots，不再通过 `interner.iter()` 把整个 module log 当语义输入。`ItemSignatures::type_roots` 同样显式覆盖 function/generic/where/aggregate/trait/impl/enum/alias/global/const signature handles；`LayoutComputer` 的普通计算从 signature 与 source type-use roots 递归布局，并继续显式计算本模块非泛型 struct/union，删除了会在 append 过程中反复复制并扫描完整 interner 的 full-scan loop。精确 executable/signature layout 保留已有的 `LayoutRoots`，不退化为全模块扫描。新增 root 参数使旧 `compute_layouts_with_normalized_types` 超过 Clippy 的合理参数边界后，该 convenience API 被整体删除，所有调用统一使用 `LayoutComputationInput`，没有 allow 或 overload。第一次原样全测准确暴露出只出现在 const `@size/@align` operand 或 imported source type-use 中的六个 roots 没有进入普通 module layout；修复让 const builtin 把实际 operand 显式加入单次 layout 请求，并让 module provider 合并 signature 与 lowering roots，没有恢复 view scan。normalization/backend/query 211 项首轮定向测试、layout/const/body/query 261 项串联测试、相关 const aggregate/layout-builtin 回归与 driver 484 项均通过；`cargo check --workspace --all-targets`、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 最终全部通过，CLI commands 50 项约 241.29 秒完成，WSL 未 OOM 或退出。Phase B 现约完成 85–90%；剩余根部是让 `TypeLowering` 不再携带 `TyInterner`，把 prefix/temporary append ownership 改为 store capability，随后删除 module visibility log、snapshot/checkout migration API、`TypeOrigin` 与 `TyInternerId`。

进展（2026-07-17）：type-lowering 与检查器的最后一批 product view 已删除。`TypeLowering` 不再携带 `TyInterner`，只发布 source-addressable type facts、const expressions 与 diagnostics；所有会内部创建孤立 store 的 `lower_module_types*` convenience/with-defs 入口整体删除，唯一入口族要求显式 `TypeLoweringContext`。compiler-query 的 normalization/layout/const/body/codegen provider 不再用 lowering snapshot 做 prefix 证明，store transaction 本身成为 append ownership contract；standalone 与测试 pipeline 也改为从同一个 `TypeStore` checkout，而不是 clone lowering view。ABI 与 flow check 只读取 canonical store，ABI 的 function/type/value 三份 interner 字段删除；item signatures 的六个旧入口收敛为单一 `ItemSignatureInput`，collector 验证 lowering handle 的 store identity，并通过短生命周期 `TypeStoreAppend` 合成 builtin/primitive/error 类型，没有 overload 或 compatibility facade。纯汇总的 `check_module_const_with_all_phases` 同时删除了无效 mutable-interner/input 参数。两次独立提交分别完成 checking view 与 signature ownership；393 项定向测试、workspace all-targets check、严格 all-targets/all-features Clippy，以及无环境变量、无线程参数的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 242.03 秒完成，filesystem/process/allocator/ArrayList/LLVM/driver/doc tests 均通过，WSL 未 OOM 或退出。Phase B 现约完成 90–95%；剩余工作不再是 semantic product 迁移，而是把仍接收 `&mut TyInterner` 的临时算法改为 canonical append capability，随后删除 module visibility log、snapshot/checkout migration API、`TypeOrigin` 与 `TyInternerId`。

进展（2026-07-17）：Function IR lowering 的临时 mutable-interner 契约已删除。唯一单函数与 batch 入口都要求显式 `FunctionTypeContext`，既有 handle 固定从 session `TypeStore` 读取，for-in optional 与 tagged-union tag primitive 等合成类型只通过 module-scoped `TypeStoreAppend` 发布；compiler-query 不再为 function lowering checkout module shard。无 type context 时静默降级 never/try/function-pointer/for-in 语义的 convenience 入口、`*_with_interner` 旧入口和内部 `Option<&mut TyInterner>` 同时删除，没有兼容双轨。回归测试直接从 canonical store 验证 synthesized optional handle，不再用 visibility-log prefix 作为正确性标准；function-lower 49、backend-lower 96、codegen 177、compiler-query 108 项定向测试通过，严格 workspace/all-targets/all-features Clippy 无 warning，无参数、无环境变量的原样 `cargo test` 全部通过，CLI commands 50 项自然并发 242.18 秒完成，driver 484、标准库 executable 集成与 doc tests 均通过。Phase B 仍未关闭：const/body/layout/monomorphization/backend 的剩余 append view 与 checkout 仍需按同一模式迁移，随后才能删除 module visibility log、snapshot/checkout、`TypeOrigin` 与 `TyInternerId`。

进展（2026-07-17）：monomorphization 的 paired input snapshot 与普通 type read/write transaction 已删除。`MonomorphizeModuleInput` 不再携带 `TyInterner`，collector 直接从 canonical `TypeStore` 解释 generic/type graph，并通过 `TypeStoreAppend` 发布实例化 structural types；compiler-query 因此删除了 function lowering 后按 module clone snapshot map 的整条数据流，测试 fixture 也不再把 detached view 合并回 session shard。nested generic 回归直接从 mono instance handle 查询 canonical kind，不再断言 visibility-log prefix 增长；失去语义的 module 参数也从 generic-presence 与 instance-depth 整条递归链删除，没有 Clippy allow。monomorphize 9、compiler-query 108、backend-lower 96、driver 484 项定向测试通过，严格 workspace/all-targets/all-features Clippy 无 warning，无参数、无环境变量的原样 `cargo test` 全部通过，CLI commands 50 项自然并发 237.73 秒完成，标准库 executable、LLVM 与 doc tests 均通过。`TypeOrigin` 驱动的 identity-preserving recursive import 与 projection trait-solver append transaction 仍留在下一切片，本切片不将 Phase B 标为完成。

进展（2026-07-17）：monomorphization 的 `TypeOrigin`/recursive import 与 trait-solver mutable-interner 契约已删除。generic instance expansion 直接复用 canonical self/type arguments，原先按 pointer/array/function/nominal/trait-object/projection 递归重建同一 type graph 的整段代码移除；mono 生产和测试路径都不再查询 origin、snapshot 或 module view。`TraitSolverTypeCx` 只持有 canonical store 与 `TypeStoreAppend`，`solver*` 构造器不再接收 `&mut TyInterner`；const/body/program-signature/backend/mono 的全部调用点随之删除空 mutable borrow，Clippy 暴露的无效 module/view 参数和字段也一并移除，没有 compatibility overload。backend/body/query/driver/mono/signature/trait/const 定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 236.32 秒完成，标准库 executable、LLVM 与 doc tests 均通过。Phase B 仍未关闭：const/body/layout provider 的 append view、backend checkout 以及 `nia-ty` module visibility/migration 层仍需继续迁移。

进展（2026-07-17）：layout 的 mutable-interner 输入契约与 provider checkout 已删除。`LayoutComputationInput` 只接收 canonical `TypeStore`、显式 semantic roots 与 layout facts；`LayoutTypeCx` 从 store 读取既有 handle，并只通过 module-scoped `TypeStoreAppend` 发布 generic substitution 产生的 structural types。普通、signature、executable 与 checked-module layout provider 不再围绕 root collection 或 layout computation checkout module shard；`LayoutRootCollector` 显式接收 owning `ModuleId`，测试也改为从 explicit roots/canonical store 断言，不再扫描 visibility-log snapshot。layout/const/body/backend/query 357 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 237.01 秒完成，标准库 executable、LLVM、driver 与 doc tests 均通过。Phase B 仍未关闭：const/body 的阶段 append view、backend whole-program checkout 以及 `nia-ty` module visibility/migration 层仍需继续迁移。

进展（2026-07-17）：const-check 的 phase/typed-query mutable-interner 契约与 provider transaction 已删除。array-length、enum-value、value、typed-fact 和完整 const 入口只接收 semantic `ConstInput`，`TypedConstQueryInput` 也不再暴露 body checker 的 working interner；`ConstTypeCx` 从 canonical `TypeStore` 读取，并为当前及跨模块执行 context 持有 module-scoped `TypeStoreAppend`，不再区分 session/snapshot append view。full/signature/executable const provider 只构造 query inputs，不再 checkout module shard；standalone layout/static/body/backend fixtures 同步使用同一 API。旧 query 回归不再要求 visibility-log 按 phase 增长，而是直接验证 synthesized range handle 已发布到 canonical store。const/body/static/layout/query/backend 364 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 239.80 秒完成，标准库 executable、LLVM、driver 与 doc tests 均通过。Phase B 剩余生产 `&mut TyInterner` 已收缩到 body-check 三个入口和 `nia-ty` migration API；随后还需删除 backend whole-program checkout 与 module visibility/migration 层。

进展（2026-07-17）：body-check 的三个 mutable-interner 入口、session/snapshot append view 与 provider checkout 已删除。`BodyTypeCx` 从 canonical `TypeStore` 读取并通过 module-scoped `TypeStoreAppend` 发布推断、替换和 coercion 类型；standalone、backend 和 executable provider 都只传 `BodyCheckInput`。增量 `BodyCheckSeed` 现在只携带可复用的 `SemanticFacts`，不再为 prefix 证明额外 clone module snapshot；type-comparison probe 也共享 canonical capability，不复制临时 view。旧 body query 回归改为从 `SemanticFacts::local_types` 取得 synthesized array handle 并直接查询 store。body/query/backend/driver 827 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 236.05 秒完成，标准库 executable、LLVM、driver 与 doc tests 均通过。workspace 生产代码中的 `&mut TyInterner` 现只剩 `nia-ty` 自身 migration API；Phase B 仍需迁移 backend whole-program checkout 和 normalization/type-lowering fixture transaction，再删除 module visibility log、snapshot/checkout、`TypeOrigin` 与 `TyInternerId`。

进展（2026-07-18）：backend whole-program checkout 与 normalization 调用方的 mutable append 契约已删除。`BackendTypeContext` 现在只持有 module-scoped `TypeStoreAppend`，whole-program fixed point 不再取走或归还 module shard；backend instance normalization 从 signatures 与 concrete struct/union args 构造显式 roots，既不扫描 visibility log，也不借用 backend append capability。`TypeNormalizationInput` 只包含 canonical store、module、roots 与 signatures，normalizer 内部自行创建短生命周期 append capability；query、standalone 和测试调用点不再包裹 migration transaction。旧 backend 回归改为直接从 canonical store 验证 const-generic array 与 nested generic pointer，不再要求 synthesized type 出现在 module snapshot。backend-lower 96、compiler-query 108、type-normalize 7 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 234.20 秒完成，标准库 executable、LLVM、driver 与 doc tests 均通过。Phase B 剩余生产 mutable-interner 已收缩到 type lowering 与 `nia-ty` migration API；下一切片应把 type lower 改为 canonical read + append，随后删除 module visibility log、snapshot/checkout、`TypeOrigin` 与 `TyInternerId`。

进展（2026-07-18）：type lowering 的最后一个生产 mutable-interner transaction 已删除。`TypeLowerer` 显式持有 canonical `TypeStore` 读取源和 module-scoped `TypeStoreAppend` 发布能力；primitive、nominal、projection 与 structural type 全部直接进入 session store，类型等价、整数分类和 trait 解构不再从 visibility log 读取。compiler-query 的 update/normalization 回归也改为验证旧 handle 持续可由 canonical store 解释及新 roots 已发布，不再比较 snapshot prefix 或 log 长度。type-lower 12、compiler-query 108 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 236.68 秒完成，标准库 executable、LLVM、driver 与 doc tests 均通过。生产代码中的 `&mut TyInterner` 现只存在于 `nia-ty` 自身 migration API，所有 pass 的 append 契约均已跨过 view 边界；下一切片应移除 const 对非语义 `TypeOrigin` 的依赖并整体删除 module log、snapshot/checkout、`TyInternerId` 与 view 测试工具。

进展（2026-07-18）：非语义 `TypeOrigin` 已从 const、`nia-ty` 和 `nia-ids` 删除。const trait/normalization/substitution 统一以当前执行模块作为可见性上下文，nominal layout/field offset 只以真实 `GlobalDefId.module_id` 选择定义模块，不再让并发或查询顺序决定的 first-insert metadata 参与语义。canonical slot 分配直接由 canonical map 长度推进，`TypeStoreAppend` 只携带共享 core；nia-ty origin 回归随实现一起删除。const-check 5 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 238.55 秒完成，跨模块 const/trait/layout、标准库 executable、LLVM、driver 与 doc tests 均通过。Phase B 只剩无生产消费者的 module visibility/migration 实现层及其测试 fixture，下一切片应整体迁移测试并删除 `TyInterner`、`TyInternerId`、snapshot/checkout 与 same-shard guard。

进展（2026-07-18）：Phase B 的 view/migration 实现层已完整删除。LLVM、backend、function IR/opt、driver、compiler-query 与 nia-ty fixture 全部改用 `TypeStore`/`TypeStoreAppend`；`TypeStore` 不再维护 module map，`TyInterner`、module visibility log、snapshot/checkout、same-shard guard 与旧 `TyInternerId` 均从实现删除，global slot newtype 同步更名为 `TypeStoreIndex`。nia-ty 回归只保留 canonicalization、跨 module capability 同 ID、跨 session dependency 拒绝、same-session composition 与 64-bit handle 不变量。旧符号与 API 的 production/test 全仓搜索为零；nia-ty/backend/LLVM/compiler-query 390 项定向测试、workspace all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过。CLI commands 50 项自然并发 238.36 秒完成，标准库 executable、LLVM、driver 与 doc tests 均通过。Phase B acceptance 至此完成，下一提交开始 Phase C：先量化并删除 query cache hit 的默认 owned clone，再收敛 `query`/`query_shared` 双入口。

进展（2026-07-18）：Phase C 的 cache ownership 契约开始落地。`QueryKey::Value` 与 erased cache slot 不再要求 `Clone`；正式共享入口统一命名为 `get/try_get`，首次计算把 value 唯一放入 cache-owned `Arc`，cache hit 只复制句柄。不可 `Clone` value 与同 allocation cache-hit 回归直接验证该契约。旧 `query_shared/try_query_shared` 实现和全仓 Rust 调用已删除；loader-query 的 production/test 调用链进一步全部迁到 `get/try_get`，`ParsedModuleQuery`/`SyntaxModuleQuery` 不再把 runtime handle 重复声明为 `Value = Arc<T>`，只有组装公开 owned `LoadedProgram` DTO 时执行显式边界 clone。nia-query 20 项与 loader-query 36 项定向测试、loader all-targets check、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 235.36 秒完成，driver 484、LLVM 177 与 doc tests 均通过。compiler-query 仍有大量 owned `query`，runtime 也暂时保留方法级 `Value: Clone` 的 `query/try_query/query_many` output adapter，因此 Phase C 尚未达到单入口 acceptance；下一切片应按 compiler product 调用链迁移并删除该 adapter，而不是增加兼容 facade。

进展（2026-07-18）：compiler resolution product 链已迁到 cache-owned handle。`ValueResolutionQuery`、`LocalResolutionQuery` 与 `SemanticUseTableQuery` 的 production/test 消费者全部改用 `get`，const/static/module/body provider 的 cache hit 不再复制 resolution maps。`BodyCheckResolutionInputs` 对完整查询结果直接保存 `Arc`，filtered executable 路径只为本次新计算结果创建一次 handle；`CheckedModule` 的 `value_resolution`、`local_resolution` 与 `semantic_uses` 字段也改为共享 handle，普通 checked module 不再为 aggregate DTO 深拷贝三个大产品，type-only module 则直接创建空 handle。新增回归用 `Arc::ptr_eq` 验证 checked module 与三个 query slot 复用同一分配；相关 key 的旧 owned 调用搜索为零。compiler-query 109 项定向测试、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 232.04 秒完成，driver 484、LLVM 177 与 doc tests 均通过。其余 type/const/layout/body/checked program 产品和 `query_many` 仍在 owned 路径，不能据此宣称 compiler-query 已完成迁移。

进展（2026-07-18）：标准 type resolution/lowering 产品已完成共享迁移。`TypeResolutionQuery` 与 `TypeLoweringQuery` 的全部 production/test 消费者改用 `get`，普通 `CheckedModule` 直接保存 query slot 的 `Arc<TypeResolution>`/`Arc<TypeLowering>`，type-only aggregate 同样复用 signature query handle；const、semantic-use、layout、body 与 backend 输入均只借用共享产品。产品身份回归扩展为同时验证 value/local/semantic-use/type-resolution/type-lowering 五个 checked-module 字段与对应 cache slot `Arc::ptr_eq`，两个标准 type key 的 owned 调用搜索为零。迁移中发现 type normalization 仍通过 body/const 的跨模块 `Fn(ModuleId) -> Option<TypeNormalization>` owned callback 传播；本切片没有用显式 clone 掩盖它，normalization 将作为独立所有权链重构。compiler-query 109 项定向测试、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 228.40 秒完成，driver 484、LLVM 177 与 doc tests 均通过。

进展（2026-07-18）：immutable check product 的下一组 owned 路径已删除。`ConstQuery`、`AbiCheckQuery`、`StaticCheckQuery` 与 `FlowCheckQuery` 的 production/test 消费者全部改用 `get`；普通 `CheckedModule` 直接保存四个 cache handle，filtered executable 与 type-only aggregate 只为本次独立计算或空产品创建一次 `Arc`，不再复制查询结果。产品身份回归继续验证四个字段与对应 query slot 同 allocation，四类 key 的旧 owned 调用搜索为零。`BodyCheck` 包含随后要转移的 IR，不能在 Phase C 中盲目共享；`LayoutsQuery` 与 normalization 一样仍受跨模块 owned callback 约束，本切片明确保留它们等待契约级重构。compiler-query 109 项定向测试、严格 workspace/all-targets/all-features Clippy，以及无参数、无环境变量的原样 `cargo test` 全部通过；CLI commands 50 项自然并发 229.62 秒完成，driver 484、LLVM 177 与 doc tests 均通过。

进展（2026-07-18）：type normalization 产品的 owned 传播链已完整迁移。const/body/program-signatures 的跨模块 normalization callback 统一返回 cache-owned `Arc<TypeNormalization>`，对应内部 cache 只保存共享句柄；const analyzer 的本模块路径借用当前 normalization、跨模块路径共享 query 产品，不再 clone 完整 `TypeNormalization` 或 `normalized` map。extension signature aggregate 与普通 `CheckedModule` 同样直接保存 query handle，标准、signature、signature-const 与 layout-normalization 的 production 消费者全部改用 `get`。产品身份回归扩展为验证 checked module normalization 与 query slot `Arc::ptr_eq`，旧 owned callback 与 production `query(*TypeNormalizationQuery)` 搜索为零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 109、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 229.73 秒完成。下一切片迁移 layout callback 与 aggregate 产品，不能用 callback 内显式 clone 适配旧契约。

进展（2026-07-18）：layout 产品的 owned callback 与 aggregate 传播链已删除。`nia-layout`、body-check、const layout builtin 与 compiler executable fixed-point 的跨模块 callback 统一返回 `Arc<Layouts>`；精确 executable layout 在计算边界只创建一个 handle，round/final/codegen cache 与 body input 后续只复制句柄。普通 `CheckedModule` 直接保存 `LayoutsQuery` cache handle，rooted/type-only 等独立计算路径显式创建单一 handle；标准与 signature layouts 的 production/test 消费者全部迁到 `get`。产品身份回归继续验证普通 checked module 与 layout query slot 同 allocation，旧 owned layout callback、owned cache 与 `query(*LayoutsQuery)` 搜索为零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 109、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 238.14 秒完成。下一切片应审计 body/IR 产品的消费语义，不能因 `BodyCheck` 同时包含 diagnostics、facts 与可转移 IR 就直接把整个 aggregate 机械包进 `Arc`。

进展（2026-07-18）：`BodyCheckQuery` 的大产品已按真实消费语义拆分共享，而不是把可变 executable aggregate 整体机械包装。`BodyCheck` 的 `BodyIr`、`SemanticFacts`、provider demands 与 diagnostics 分别成为 cache-owned handle；普通 `CheckedModule` 直接复用四个 allocation，所有 `BodyCheckQuery` 消费者统一迁到 `get`，产品身份回归覆盖四个字段。executable fixed-point 仍以 owned `ExecutableFactModuleState` 做增量 retain/extend，在 fresh body product 进入该状态时显式 `Arc::unwrap_or_clone`，随后 final/codegen module 再以单一 handle 发布；因此标准 cache hit 不深拷贝 IR/facts，同时没有把共享可变性引入增量算法。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 109、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.79 秒完成。下一切片继续处理 `CheckedModuleQuery/query_many` 与 function/backend IR 聚合，不能让 aggregate owned adapter 重新复制刚共享的字段。

进展（2026-07-18）：并行 cache-owned 批量读取入口 `get_many` 已建立，支持非 `Clone` query value、保持 key 顺序，并沿用 `query_many` 的 logical parent stack 与 dependency merge 语义；回归验证重复非 Clone key 只执行一次且返回同一 allocation。普通 checked-program 物化改为 `get_many(CheckedModuleQuery)`，公开 `CheckedProgram`/`CodegenProgram` 与内部 executable checked-module store 统一保存 `Arc<CheckedModule>`，store/materialize/cache hit 都只复制句柄。aggregate 身份回归验证 checked program module 与独立 `CheckedModuleQuery` slot 同 allocation；刚完成共享的 body/type/layout/check 字段不再被上层 module clone 重新复制。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 21、compiler-query 109、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 231.14 秒完成。剩余八处 legacy `query_many` 仍集中在 program-signature、extension-provider 与 reachability facts，下一切片应按 value contract 分组迁移，不能仅改 API 名称后在消费端深 clone。

进展（2026-07-18）：program-signature 与 extension-provider 的 module facts 已从 `Value = Arc<T>` 双层 ownership 契约改为 cache 直接拥有裸产品，单项消费者统一通过 `get` 取得唯一共享句柄，program ABI/signature、trait solving、provider index、named method 与 nominal target 聚合统一通过 `get_many` 复用相同 allocation。新增 compiler-query 回归同时验证五类 facts 的单项与批量读取均 `Arc::ptr_eq`，避免把 API 迁移退化为 nested `Arc` 或消费端深 clone；compiler-query 中 legacy `query_many` 从八处降至仅剩 executable reachability lookup 两处。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 110、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 229.19 秒完成，全部 doc tests 通过。下一切片应迁移最后两处 reachability 批读取并把 compiler-query 的 `query_many` production 搜索收敛为零。

进展（2026-07-18）：executable reachability 的 trait 与 trait-method extension lookup 已改用 `get_many`，每个 provider module 直接借用上一切片建立的 cache-owned facts handle，不再经 legacy owned batch adapter clone methods、associated values、diagnostics 与 nominal provider aggregate。compiler-query production 的 `.query_many(...)` 搜索至此为零，Phase C 批读取调用面已统一到 `get_many`；runtime 的 legacy `query_many` 实现仍为兼容测试保留，后续应在完成 owned `query` 迁移时一并删除，而不能把它误记为 executor 已完成。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 110、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 228.24 秒完成，全部 doc tests 通过。下一切片回到剩余 owned `query` 产品链，优先审计 const phase 与 function/backend IR 的所有权边界。

进展（2026-07-18）：标准 const phase 的 `ConstModuleQuery`、array-length、enum-value、const-value 与 typed-facts production/test 消费者已统一改用 cache-owned `get`。layout、body、static 与 program-check 等只读路径现在只复制 `Arc`；array-length → enum → values → typed-facts 以及最终 `ConstCheck` 组装仍需要 owned mutable seed，复制点统一显式标为 `Arc::unwrap_or_clone`，没有用隐式 legacy adapter 掩盖算法所有权。五类标准 const key 的旧 owned `query` 搜索为零，但 `ConstProgramContext`/body/static 的跨模块 module/value callback 仍返回 owned product，signature const 也仍在 owned 路径，因此不能宣称 const phase 已完成共享。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 110、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 229.97 秒完成，全部 doc tests 通过。下一切片应迁移 signature const 查询产品，并随后把跨模块 const callback 改为共享句柄。

进展（2026-07-18）：signature const 的 active item tree、type resolution、type lowering、item signatures 与 module lowering production 消费者已统一改用 `get`；`signature_const_module_lowering` helper 直接返回 query cache handle，global-initializer 只读路径借用该 handle，不再为 helper 调用复制完整 lowering。标准与 signature 两组 const/module key 的旧 owned 查询搜索均为零；仅三个受 `ConstProgramContext`/body-check 旧 `Fn(ModuleId) -> Option<ResolvedConstModule>` 契约约束的跨模块 callback 仍显式 clone `module`，该复制必须通过下一次公共 callback 契约迁移删除，而不是藏进 helper。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 110、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 230.49 秒完成，全部 doc tests 通过。下一切片应把 const-check、body-check 与 static-check 的 program module/value callback 改为共享句柄，并审计 executable 临时 fact cache 的 owned 边界。

进展（2026-07-18）：const-check、body-check 与 static-check 的跨模块 resolved-module、const-values 和 array-length callback 已统一返回 `Arc`；`ConstModuleLowering` 内部直接共享 `ResolvedConstModule`，标准/signature query callback 只复制内部句柄。executable 的 signature/override 临时 const facts cache 同步改为保存 `Arc<ConstValues>`/`Arc<ConstArrayLengths>`，标准分支直接返回 query handle，只有 phase 算法取得 mutable seed 时显式 `Arc::unwrap_or_clone`。旧 `Fn(ModuleId) -> Option<ResolvedConstModule|ConstValues|ConstArrayLengths>` 与 owned executable fact cache 的 production 搜索均为零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；body-check 139、compiler-query 110、const-check 5、static-check 7、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 233.89 秒完成，全部 doc tests 通过。下一切片应让最终 `ConstCheck` aggregate 复用 phase map allocation，删除 phase → aggregate 的完整 map move/clone 边界。

进展（2026-07-18）：`ConstArrayLengths`、`ConstEnumValues`、`ConstValues`、`ConstTypedFacts` 与最终 `ConstCheck` 的事实表统一为内部 `Arc<HashMap<...>>`；phase analyzer 只在继承上一阶段并继续追加事实时 `Arc::unwrap_or_clone`，最终 aggregate 直接转交 values、typed values、enum values、typed enum values 与 array lengths 五个 allocation。compiler-query 产品身份回归以 `Arc::ptr_eq` 验证 `ConstCheck` 五张表分别复用四个 phase query slot；backend lowering 建立可变 array-length index 与故障注入测试分别显式 clone/`Arc::make_mut`，没有引入共享可变性。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 110、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.15 秒完成，全部 doc tests 通过。const facts 的 query、callback 与 aggregate 共享链至此完整，下一切片应转向 function/backend IR 产品，或继续消除 const phase diagnostics 的重复传播。

进展（2026-07-19）：executable function lowering 已成为按 `ModuleId` 缓存的独立 query 产品，并显式依赖当前 `ExecutableCheckedModuleSet`；monomorphization 的预验证与 backend lowering 通过 `get_many` 共享同一组 cache-owned `LoweredFunctionBodies` handle，不再对同一 filtered body IR 各执行一次完整 lowering。backend 输入和诊断路径只借用共享产品；trace 回归验证完整 codegen 对每个 executable module 只执行一次 function lowering，backend 阶段产生对应 cache hit。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 111、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 230.80 秒完成，全部 doc tests 通过。Phase C 现约完成 65%；下一切片应审计 `CodegenProgram` 的 monomorphization/backend 大聚合与剩余 legacy owned `query/try_query` 边界，不能以机械包裹整个可变 program aggregate 代替真实消费语义。

进展（2026-07-19）：`CodegenProgram` 的真实消费者审计确认 driver、CLI、LLVM 与测试只读取 monomorphization 和 backend lowering，不存在下游原地修改；这两个最大产品因此改为显式 `Arc` 字段，query cache 的内部 `get` 与公开 owned DTO adapter 现在只复制句柄，不再深 clone mono instance 列表、完整 `BackendProgram`、优化报告和 backend diagnostics。test-only `MonomorphizationQuery`/`BackendLoweringQuery` 消费者也统一改用 `get`；指针身份回归验证 legacy `query(CodegenProgramQuery)` 边界仍复用两个大产品 allocation。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 112、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.20 秒完成，全部 doc tests 通过。Phase C 现约完成 70%；下一切片应按产品族继续删除 compiler-query 剩余 owned `query`，最终收敛 runtime `query/try_query/query_many` adapter 并建立 declarative registry。

进展（2026-07-19）：definition/signature 大产品的剩余 owned 边界已删除。公开 `CheckedModule.defs` 与 executable 增量 `ExecutableFactModuleState.defs` 统一保存 cache-owned `Arc<DefCollection>`，普通、filtered executable 与 type-only module 构造不再深 clone module definition table；production 的 `ModuleDefsQuery`/`FullModuleDefsQuery` 和标准 `ItemSignaturesQuery` 消费者全部改用 `get`。新增回归验证普通 checked module 与 executable checked module 的 defs 都和 `FullModuleDefsQuery` slot `Arc::ptr_eq`，对应 production owned 查询搜索为零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 113、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 225.86 秒完成，全部 doc tests 通过。Phase C 现约完成 72%；下一切片应审计 runtime legacy adapter 的真实外部边界，并继续迁移仍会复制大集合的 visible-extension/public-surface 产品族。

进展（2026-07-19）：runtime owned batch adapter 已完整删除。workspace production 对 `query_many` 早已没有调用；本切片进一步移除 `QueryDb::query_many` 的重复线程队列与 `Value: Clone` 输出路径，把 key 顺序、logical parent stack、dependency merge、worker cycle、transitive invalidation 和 invalidation-during-compute race 回归全部迁到唯一 `get_many` 实现，并将内部 worker 命名从 legacy query-many 收敛为 batch。全仓 Rust `query_many` 搜索为零；`query/try_query` 仍承担 compiler public owned DTO 与 query-error 转换边界，本切片没有提前删除。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 21、compiler-query 113、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 226.69 秒完成，全部 doc tests 通过。Phase C 现约完成 75%；下一切片应迁移 visible-extension/public-surface 等剩余大集合 consumer，再把公开 DTO 从 runtime owned adapter 中拆出，最终删除 `query/try_query` 与 `OwnedQueryOutput`。

进展（2026-07-19）：visible-extension 产品族已消除 query value 中预包装的 `Arc`。`VisibleExtensionsQuery` 与 `VisibleTraitImplsQuery` 现在由 cache 直接拥有裸 `VisibleExtensionsForModule` / `VisibleTraitImplsForModule`，production 消费者统一通过 `get` 取得单层共享句柄；backend input、executable index slice 与 associated-value resolver 显式保存同一 `Arc`，不再形成 `Arc<Arc<T>>` 或经 owned adapter 深 clone methods、associated values、trait impls 与 diagnostics。显式类型和 `Arc::ptr_eq` 回归验证两类产品的单项及批量读取复用同一 allocation，对应 production owned 查询搜索为零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 113、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 230.52 秒完成，全部 doc tests 通过。Phase C 现约完成 78%；下一切片应以相同契约迁移 public-surface/public-using-scope 产品族，并审计 module using scope 的 owned map 边界。

进展（2026-07-19）：public-surface 与 using-scope 产品族已统一到单层 cache ownership。`PublicSurfacesQuery` / `PublicUsingScopesQuery` 的 value 从预包装 `Arc` 改为裸 snapshot 产品，compiler input 仍以显式 `Arc` 保留独立失效源，query miss 只从输入快照物化一次，后续 frontend、const、body、codegen 与 diagnostics 消费者全部通过 `get` 共享 cache handle。`ModuleUsingScopeQuery` 的直接和 resolver-closure 消费者也迁到 `get`，不再为每次 extension/body/const lookup clone scope map 或额外 `Arc::new`。显式 `Arc<PublicSurfacesQueryValue>`、`Arc<PublicUsingScopesQueryValue>` 与 `Arc<ModuleUsingScope>` 回归验证单项/批量读取共享同一 allocation；三类 production owned 查询搜索为零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 114、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 229.85 秒完成，全部 doc tests 通过。Phase C 现约完成 81%；下一切片应量化并迁移剩余 owned `query/try_query` 产品族，再把公开 owned DTO/error 边界从 runtime adapter 中拆出。

进展（2026-07-19）：item-tree snapshot 与 program signature index 的双层 query ownership 已删除。六类 module/declaration/full item-tree input query、`FullModuleItemTreeQuery` 与 `FullActiveModuleItemTreeQuery` 现在由 cache 直接拥有裸 tree 产品；compiler input 继续以显式 `Arc` 作为独立失效源，所有 semantic/const/body/check/codegen 消费者统一通过 `get` 共享单层句柄，snapshot dependency boundary 保持不变。`ProgramTraitMethodIndexQuery` 与 `ProgramAbiSignaturesQuery` 同样改为裸产品，body method resolver 与 ABI checker 不再经 owned adapter 或 nested `Arc`。显式 tree/index/ABI 类型和单项/批量 `Arc::ptr_eq` 回归锁定 allocation identity，十类对应 production owned 查询搜索为零；compiler-query 显式 nested-`Arc` query declaration 从 23 类降至 13 类。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 115、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.98 秒完成，全部 doc tests 通过。Phase C 现约完成 84%；下一切片应迁移剩余 extension index/value 产品族，再拆分 public facade 的 owned DTO/query-error 边界。

进展（2026-07-19）：extension discovery/index/value 产品链已完整消除 query value 内的预包装 `Arc`。provider discovery、signature input、trait solving lookup、validation、type exposure、全局/named/by-id method index 与 trait signature index 九类产品均由 cache 直接拥有裸值；reachability、body、codegen 与 extension provider 内部消费者统一通过 `get` 共享单层句柄，旧 owned 查询和 provider-side `Arc::new` 搜索归零。新增回归从 source trait/extension fixture 构造真实 trait identity，以显式九类 `Arc<裸产品>` 和单项/批量 `Arc::ptr_eq` 覆盖完整链路。compiler-query 显式 nested-`Arc` query declaration 从 13 类降至仅剩四个 `Option/Vec<Arc<实体>>` 复合值；这些是实体句柄集合，不能按文本搜索机械拆除，需在 public adapter 收敛后按所有权语义处理。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 116、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 231.65 秒完成，全部 doc tests 通过。Phase C 现约完成 88%；下一切片应把 compiler public facade 改为 `try_get` 加显式 DTO materialization，再迁移小 Copy/owned 查询并删除 runtime adapter。

进展（2026-07-19）：query runtime 的 owned 输出双轨已完整删除。`CompilerDatabase` 的 checked/entry/codegen/provider-demand public facade 统一调用 `try_get`，仅在返回现有 owned DTO 的 API 边界显式 `Arc::unwrap_or_clone`；codegen 回归验证该 materialization 仍复用 monomorphization/backend 大产品 allocation。compiler production 与测试的 lookup、小 Copy 值、诊断 DTO 和实体集合调用全部迁到 `get/try_get/get_many`，需要 owned 值的位置显式 copy/clone，缓存层不再隐式决定消费语义。`QueryDb::query/try_query`、`OwnedQueryOutput`、generic output adapter 和 clone timing 分支均已删除，cycle/panic/invalid-input/dependency/invalidation 回归改由唯一 API 验证；全仓 Rust `.query/.try_query` 与旧 adapter 搜索为零。两类 nominal provider candidate 产品同步去除最后的预包装 `Arc`；余下 `Option/Vec<Arc<实体>>` 是产品内部实体句柄，不是 query value 双层 wrapper。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 21、compiler-query 116、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 228.59 秒完成，全部 doc tests 通过。Phase C 的唯一 API、cache ownership、无默认 clone 与旧入口删除 acceptance 已满足，现约完成 96%；关闭 Phase C 前仍须建立 declarative registry，并明确剩余 module/program aggregate 的 item/body storage policy，完成后再进入 ID/arena 深度审计。

进展（2026-07-19）：declarative query registry 已落地并关闭 Phase C。`nia-query` 现在显式描述每个 query 的 key/value/context、provider、fingerprint 与 storage policy；严格 DB 在建立 typed slot 前拒绝未注册 key，同时拒绝重复 key type 和重复 query name，轻量 runtime 单测仍可选择 permissive DB。compiler 生产 registry 覆盖 113 个 query，测试构建额外覆盖 3 个 test-only query；loader registry 覆盖全部 10 个 query，生产与直接构造 DB 的测试均启用严格模式。完整性回归锁定描述符数量、唯一稳定名称以及当前真实的 `KeyExecute` / `None` / `CacheOwnedArc` 策略，不把 Phase D 尚未实现的 fingerprint 虚报为 stable。聚合产品审计确认 `CheckedProgram` / `CodegenProgram` 已由轻量顶层汇总和独立 module/item/body handles 组成；剩余四类 `Option/Vec<Arc<Entity>>` 是实体身份集合而非 nested query wrapper，最终 storage policy 已记录，避免为了文本上“去 Arc”重新引入深拷贝。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 24、loader-query 37、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.17 秒完成，全部 doc tests 通过。Phase C 至此 100% 完成；下一阶段按约定先做 ID/arena 深度审计并形成独立后续计划，再决定是否直接进入 Phase D。

### 阶段 C（P0）：重做 query value/storage 契约

1. 去掉通用 `Value: Clone` 要求。
2. 公开调用统一为一种 `get::<Q>(key)` 语义，调用者不能选择 owned/shared 路径。
3. cache/store 唯一拥有 value；arena ref、typed handle 或小值 Copy 是 runtime 内部 storage policy。
4. 把 module/program aggregate query 拆为 index + item/body handle。
5. 建立显式 declarative query registry，记录 key/value/provider/fingerprint/storage；代码生成只允许机械消除 glue，不隐藏依赖语义。
6. 删除 `query`、`query_shared` 等旧入口和兼容 adapter。

Phase C 最终 storage policy（2026-07-19）：query cache 以唯一的外层 `Arc<Value>` 拥有每个产品；module/program aggregate 只保存顺序、索引、诊断等轻量汇总，以及由独立 module/item/body query 产生的共享 handle。`CheckedProgram` / `CodegenProgram` 的 module 列表复用 `Arc<CheckedModule>`，而 `CheckedModule` 继续复用 defs、resolution、const/layout/body 等细粒度产品；monomorphization、backend lowering 与 lowered function bodies 同样保持独立 allocation。剩余 `Option<Arc<Entity>>` / `Vec<Arc<Entity>>` 是产品内部的实体身份集合，不是 `Arc<Arc<Product>>` query wrapper，不应机械改成 owned clone；需要独占修改的 phase 必须在算法边界显式 `Arc::unwrap_or_clone` / `Arc::make_mut`。当前 declarative registry 如实记录 `KeyExecute` provider、`CacheOwnedArc` storage 和尚未实现的 `None` fingerprint；stable fingerprint 属于 Phase D，不能在元数据中虚报。

Acceptance：所有调用点使用同一查询入口；cache hit 不深拷贝；clone instrumentation 中 compiler product clone 接近零；旧 API 删除，不保留双轨。

### Phase C 后 ID/arena 深度审计（2026-07-19）

结论：ID 与 `Arc` 解决的不是同一个问题。ID 表达可比较、可序列化或高扇出的逻辑身份，必须由明确 owner/store 解释；`Arc` 表达 immutable allocation 的共享生命周期。只有当同一实体被许多表、边或产品反复引用，并且已有唯一 owner、失效和 stale-handle 规则时，ID 化才会减少内存与 clone。对没有 canonical entity identity 的粗粒度 query product，cache-owned `Arc` 仍是正确 storage；把它机械改成 `FooId -> Arc<Foo>` 只会增加一次 lookup 和一套生命周期。

本轮实现审计得到以下决策：

| 域 | 当前事实 | 决策 |
|---|---|---|
| type | `InternedTyId` 已是带 `TypeStoreId` 的 8-byte session handle，store append-only 且拒绝 foreign handle | 保持现状，不再叠加 module/type-product ID |
| query dependency | `QueryFrameIdentity` 同时携带 `TypeId`、name、hash、`Arc<dyn ErasedQueryKey>` 和 frame function，并作为 key/value 复制到 forward/reverse graph 与 query stack | P0：slot 首次建立时分配紧凑 `QueryNodeId`；typed/erased key 只在 slot identity table 保存一份，依赖边、执行栈和 invalidation worklist 只传 ID |
| syntax node | `VersionedNodeKey` 内嵌 source/revision/kind 和 span 或拥有 `Vec<u32>` 的 child path；workspace 有 59 张 `HashMap<VersionedNodeKey, ...>`，其中 `nia-sema-ir` 29 张，body/resolve/lower 路径还会频繁 clone key | P0：建立 session-owned append-only `NodeStore`，以 `NodeStoreId + NodeIndex` 的 8-byte `NodeId` 作为 AST/semantic hot-path key；结构化 locator 只在 store、诊断和 persistence 边界保留 |
| source/path | `SourceId` 已存在，但 loader query key 仍携带 owned `SourcePath(String)`；path、identity 和 query frame 会重复 normalize/clone | P0/P1：source store 统一分配 opaque handle，已注册 source 的 text/syntax/parse query 改用 `SourceId` / `SourceVersion`；`SourcePath` 只留在发现、诊断与 stable-key 边界 |
| module graph | `ModuleId` 已是自然身份，但仍是公开 tuple，仓库有约 710 处数字构造；full graph 在 loader DTO、compiler input 和 checked/codegen DTO 间深 clone，full `ModuleNode` query 又返回 `Arc` | P1：把 `ModuleId` 改为 allocator-owned opaque handle并建立 `StableModuleKey` 映射；内部只传 `ModuleId` 和精确 field query，删除 `ModuleNodeRef::Shared` / `Arc<ModuleNode>`；公开快照共享 `Arc<ModuleGraph>`，不新增重复的 `ModuleNodeId` |
| definitions/signatures | definition 已由 `GlobalDefId` 定位，`DefMap` 用 dense vec + id index；program signature 仍在多张 `HashMap<GlobalDefId, owned signature>` 间 clone/merge | P1：以 `GlobalDefId` 作为唯一 item identity，逐步改成 per-item query/typed store handle；不新增 `DefCollectionId`、`ProgramFunctionSignatureId` 等现有 key 的别名 ID |
| checked/query products | `CheckedModule`、`DefCollection`、public surface、layout/const/body facts 都是 immutable revisioned query products，且已有 `ModuleId` / `GlobalDefId` query key | 保留 cache-owned `Arc`；通过更细 key、handle aggregate 和 query invalidation控制生命周期，不制造 product ID table |
| executable set | 当前已有 `ExecutableCheckedModuleSetId`，但 store 是 `RwLock<HashMap<SetId, HashMap<ModuleId, Arc<CheckedModule>>>>`，update 时 clear，消费者仍返回 `Arc` | P1：在 compilation arena 建立后改为带 epoch/generation 的 `(set, module)` handle；在此之前不把内部临时 ID 扩散为公共 API |
| IR/codegen | body 已由 `GlobalDefId`、local/block/scope 已由 typed ID 定位；优化 pass 需要独占修改 | source body 继续以 definition/query key 定位；只为真正同时存在的 substitution/CGU/work product 引入 `MonoItemId` / `CodegenUnitId`，可变 IR 保持 owned transfer，不用共享 arena 掩盖所有权 |
| diagnostics | 各 phase 仍分别保存和聚合 `Vec<Diagnostic>` | P2：统一 session diagnostic store 后再引入 `DiagnosticId` / bundle handle；不能先造 ID 再继续复制 payload store |

所有新 handle 必须满足统一不变量：不同语义域使用不同 newtype；append-only store 不复用 slot，会回收的 store必须带 generation；debug/test 必须拒绝 foreign owner 和 stale generation；local hot-path ID 与跨 session stable key 分离；解析必须经过显式 owner capability，不能依赖 process-global table；若一个对象已经由 `ModuleId`、`GlobalDefId` 或 query key 唯一定位，就不得再增加同义 ID。每次迁移都同时记录 `size_of`、allocated bytes、clone count 和 query executions，不能只用 API 更短作为收益证据。

执行顺序：

1. **ID-0 / Query node arena。** registry 为 query kind 提供紧凑编号；slot table 分配 `QueryNodeId`，dependency graph、query stack、cycle detection 和 invalidation 改存 ID。保留 typed key equality 处理 hash collision，但每个 slot 只保留一份 erased key。模型/回归必须覆盖同 hash 异 key、递归、跨 worker cycle、计算中失效和 deterministic trace materialization。
2. **ID-1 / Source + syntax identity。** 建立 loader/compiler 共享的 source/node owner；先把 loader 的 text/syntax/parse key 从 `SourcePath` 收敛到 source handle，再把 AST、item tree、origin table和 semantic side tables原子迁到 8-byte `NodeId`。结构 locator -> local ID 的映射按 `SourceVersion` append-only，旧 revision slot 不改义；持久缓存使用 locator/stable key，不保存本次 session index。
3. **ID-2 / Module identity 与 graph snapshot。** 将 `ModuleId` 字段私有化，由 graph allocator 创建；测试统一通过 fixture/graph 获得真实 ID。引入 stable module key 映射，删除魔法 `ModuleId(0)` 和按裸 index 猜入口；逐步删除 full-node query/shared ref，loader/compiler/public DTO 共享 immutable graph snapshot。
4. **ID-3 / Item/body storage。** program signatures 改为 `GlobalDefId` keyed item product，module/program 只保存 id set/index；评估 `TypedBody(GlobalDefId)` 与 lowered body/mono instance 的实际并存关系，只在一对多实例域引入新 ID。迁移后删除 owned signature merge、module body aggregate clone 和兼容 callback。
5. **ID-4 / Executable、diagnostic 与 work products。** executable set 使用 generational arena handle；diagnostics集中存储；随后定义 `MonoItemId`、`CodegenUnitId` 和可持久化 work-product key，明确创建、消费、释放与序列化边界。
6. **接入 Phase D。** `QueryNodeId` 是进程内 dep-node，stable query key/fingerprint 是跨 revision/进程身份；两者映射后实现 red-green validation。source/module/node 的 stable/local 映射成为 loader/compiler 统一 fact graph 的 identity layer，不再新增第三套摘要 ID。

专项 Acceptance：dependency edges 与 query stack 不再复制 erased key；普通 `NodeId` 为 8 bytes，semantic facts 不再以结构化 `VersionedNodeKey` 为 map key；loader 热 query key 不含 owned path string；`ModuleId` 不可在 allocator 外构造且 stale/foreign handle 有自动化验证；full graph/module node 的 compiler product clone bytes 为 0；新增 ID 后 lookup、allocated bytes、RSS 与 clean/incremental 等价性至少不退化。每个切片继续通过严格 workspace/all-targets/all-features Clippy、完整 workspace tests 和既有 perf guard。

进展（2026-07-19）：ID-0 query node arena 已完成。每个 `QueryDb` 现在分配独立 `QueryDbId`，typed cache 首次建立 slot 时从 append-only slot table 获得 8-byte `QueryNodeId { db_id, index }`；query stack、batch worker dependency merge、forward/reverse dependency graph、cycle detection、transitive invalidation 和 invalidation worklist 全部只复制该 Copy handle，不再复制包含 `Arc<dyn ErasedQueryKey>` 的 `QueryFrameIdentity`。typed cache 改为 `Arc<K>` key，slot identity table 与 typed lookup 共享同一 key allocation；erased key 只在 slot record 保留一份用于按需 materialize trace/error frame。跨 DB node id 不相等，跨 DB 嵌套读取不会误记为本 DB dependency；trace 和 invalidation 仍按 materialized frame 稳定排序，而不是依赖并发 slot 分配顺序。第一次全测暴露旧契约中“未缓存 key 的 invalidation 仍报告 root frame”；实现现保留该行为但不为它分配 slot，并有独立回归，两个 compiler 精确失效用例恢复。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 26、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 228.68 秒完成，全部 doc tests 通过。下一切片进入 ID-1，先建立 source/node identity owner 与尺寸/foreign/stale 不变量，再迁移 loader hot query key；不能直接把结构 `VersionedNodeKey` 全局替换为无 owner 的裸 `u32`。

进展（2026-07-19）：ID-1a 的 session node identity owner 已建立。`nia-node-id` 新增带独立 `NodeStoreId` owner 的 8-byte `NodeId { store_id, index }`，append-only `NodeStore` 只在 canonical locator table 中保存一份 `VersionedNodeKey`，写 capability 通过 `NodeStoreAppend` 显式分离；同 locator 重复 intern 返回同一 handle，不同 source revision 分配不同且永不改义的 slot，跨 store handle 在解析时被拒绝。`VersionedNodeKey` 继续作为诊断、持久化和跨 session 的结构 locator，没有被无 owner 的裸整数替代。尺寸、canonicalization、revision stability 与 foreign-owner 回归已覆盖。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-node-id 9、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 233.58 秒完成，全部 doc tests 通过。下一切片把 loader 的 source text、parse、syntax、declaration/provider/facade 热查询键从 owned `SourcePath` 收敛到 `SourceId` / `SourceVersion`，之后再让 AST/semantic side table 原子采用 `NodeId`。

进展（2026-07-19）：ID-1b 的 loader source identity 迁移已完成。`SourceTable` 现在同时维护 path → id 与 id → canonical `Arc<SourcePath>`，缺失文件也会在读取前获得同一 session source handle；路径只在发现、文件系统、DTO 与诊断边界解析。`SourceTextQuery` / `LoadedModuleQuery` 改为 4-byte `SourceId` key，parse、syntax、declaration、provider summary 与 facade facts 五类 revisioned query 改为 16-byte `SourceVersion` key，七类热查询声明均不再拥有 path string；path helper 改为借用，graph/provider 热路径不再为了构造 key 深 clone `SourcePath`。`set_source` 与 public invalidation 统一失效 `SourceTextQuery(SourceId)`，missing-file 诊断、source-dependent transitive invalidation 和 query trace 依赖保持精确。尺寸回归锁定全部 key，reverse lookup、unknown id 与 graph 外 source 输入均有回归。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-loader-query 38、nia-source 8、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.51 秒完成，全部 doc tests 通过。下一切片把 session `NodeStore` 接入 source/syntax ownership，并从 `NodeOriginTable` 与 item-tree/semantic side table 开始原子迁移 `VersionedNodeKey`；在 owner capability 接入前不扩散裸 `NodeId`。

进展（2026-07-19）：ID-1c 已把 session node owner 接入 loader/parser origin 链。每个 `LoaderContext` 现在持有唯一 append-only `NodeStore`，所有 revisioned parse query 都把 AST locator intern 到该 store；`NodeOriginTable` 的 hot map value 从拥有 child-path/range 的 `VersionedNodeKey` 改为 8-byte `NodeId`，结构 locator 只通过显式 `locator` 边界 materialize。构建期写权限隔离在 `NodeOriginTableBuilder`，cache/public query product 只保留 read-only store 与 ID map；跨 store table equality 仍按 locator 语义比较，避免测试/独立 parser session 因 local handle 不同产生伪变更。增量回归验证 source 更新后新 revision 分配新 handle、两版 origin 属于同一 loader store且旧 slot locator 永不改义。roadmap 顶部状态同时刷新：Phase C 已关闭，总体约 35%，Phase A 剩余 CI/trend storage 明确不阻塞当前 identity/fact-graph 临界路径。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-node-id 10、parser 85、loader-query 38、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 233.67 秒完成，全部 doc tests 通过。下一切片应让 item-tree 顶层 node 与 definition/origin lookup 共享同一 `NodeId`，再以该 owner capability 原子迁移第一组 semantic side tables；不能在 AST/item-tree/defs 三者之间保留永久双轨。

进展（2026-07-19）：ID-1d 已把完整 definition node lookup 从结构 locator map 迁到 session-local ID map。`DefNodeMap` 现在只保存 `HashMap<NodeId, DefId>` 和 read-only `NodeStore`，collector 通过独立 builder intern 顶层 item、field、variant、method、binding 等全部 definition locator；现有 AST/item-tree 消费者仍可在显式 lookup 边界用 `VersionedNodeKey` 解析，entries 仅在检查/诊断边界 materialize locator。`CompilerContext` 持有独立 compilation-session node owner，普通/full defs query 共享它而不依赖 `ModuleOriginsQuery`。第一次定向验证准确暴露出若让 defs 读取 origin query，纯 body/revision 更新会误清 definition、signature 与 extension provider 缓存；owner 提升后五个精确失效回归全部恢复，trace 同时锁定 `module_defs → active_item_tree` 依赖存在且 `module_defs → module_origins` 依赖不存在。defs 的 node ID/store identity 与 locator round-trip 也有回归。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-node-id 10、nia-defs 6、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 232.24 秒完成，全部 doc tests 通过。下一切片应抽取同一 owner-scoped node-map 契约并迁移 `SemanticUseTable` 的六张结构 key map，随后再进入 body facts；不能为每张 map 重复实现 owner/equality/materialization 逻辑。

进展（2026-07-19）：ID-1e 已建立可复用的 owner-scoped `NodeMap<V>` / `NodeMapBuilder<V>`，统一承担 `NodeId` hot map、builder-only 写入、locator/handle lookup、显式 locator materialization 与跨 store locator 语义相等；`DefNodeMap` 已改为薄封装，不再复制 owner/equality/iteration 实现。`SemanticUseTable` 的 value use、const generic use、builtin associated value、associated const projection、local def 与 type use 六张主表已从 `HashMap<VersionedNodeKey, V>` 原子迁到共享 compiler session owner 的 `NodeMap<V>`，独立 crate/test builder 仍可创建隔离 owner；body checker 向可变执行事实复制时显式 materialize locator。semantic query 直接使用 `CompilerContext::node_store()`，没有新增 `ModuleOriginsQuery` 依赖；trace 锁定该边界，六图共同 store、foreign handle 拒绝、locator/ID 往返和跨 owner equality 均有回归，五个既有精确缓存场景继续通过。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-node-id 13、nia-sema-ir 3、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 312.24 秒完成，全部 doc tests 通过。ID/arena 专项现约 35%；下一切片应按 owner 与消费边界分组迁移 `SemanticFacts` / `FunctionSemanticFacts` 的 body node maps，避免一次性把可变 check-time facts 和 cache-owned immutable facts混成同一生命周期。

进展（2026-07-19）：ID-1f 已按生命周期边界迁移全部 per-function body node facts。`FunctionSemanticFacts` 的 expr type、bracket resolution、array-to-slice coercion、trait-object coercion/upcast、builtin value、associated const projection、array repeat count、switch value、resolved call 与 function reference 十一张 cache-owned 只读表现统一使用 `NodeMap<V>`；检查期显式改用 `FunctionSemanticFactsBuilder` 的 mutable locator maps，`BodyCheck::finish` 才以 `SemanticUseTable` 的 compiler session owner 冻结。增量 executable prechecked 路径通过 consuming `into_builder` 在唯一重用边界解冻，BIR lowering 不承担隐式 lookup/materialization；`NodeMap` 同步增加 consuming entries、keys 与 read-only owner 边界。模块/函数联合迭代现在显式 materialize locator，reachability/layout/backend 的只读 value lookup保持 ID hot path。回归锁定十一图共享 owner、freeze/thaw locator 往返、跨 owner 语义相等，以及 checked module 的 function facts 与 semantic uses 共用 store；compiler-query 117 项包含两条 prechecked 增量 body 回归均通过。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-sema-ir 4、body-check 139、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 308.46 秒完成，全部 doc tests 通过。ID/arena 专项现约 40%；下一切片应为 module-level `SemanticFacts` 引入独立 mutable builder/freeze 边界，迁移剩余 node maps并让 `SemanticFacts::extend` 在 consuming owner-aware 路径合并，随后关闭 ID-1。

进展（2026-07-19）：ID-1g 已冻结 module-level `SemanticFacts` 的十二张 node map。body checker 现在先在 `SemanticFactsBuilder` 中完成模块/函数重复事实去除，再以 semantic-use session owner 一次冻结；cache-owned `SemanticFacts` 只保存 `NodeMap<V>`。executable incremental `extend` 使用 consuming `NodeMapBuilder::extend_map`：同 owner 直接搬运 `NodeId` entries并保留原 handle，跨 owner 才 materialize locator/rehome，且嵌套 `FunctionSemanticFacts` 同步归一到目标 owner；reachable filter 则保留原 store并只移动需要的 frozen maps。prechecked 路径先读取 global type facts，再在唯一边界 consuming 解冻，不恢复双轨 cache product。回归覆盖 same-owner handle stability、foreign map rehome、module/function 十二加十一张图共同 owner、freeze/thaw equality，以及 checked module 的 semantic use/module/function facts store identity。closure audit 同时确认 ID-1 仍有 `ValueResolution` 五张、`LocalResolution` 两张、`TypeResolution` 一张 cache-owned locator map；body checker与 sema builder 中的 locator HashMap 是合法短生命周期，`NodeStore.by_locator` 是 canonical stable boundary。首次完整测试因历史 `/tmp/nia_*` 累积耗尽 3.9 GiB tmpfs，在清理 15,000+ 个测试临时目录后，精确失败用例与无参数 `cargo test --workspace` 完整重跑均通过；严格 workspace/all-targets/all-features Clippy 无 warning，nia-node-id 14、nia-sema-ir 5、body-check 139、compiler-query 117、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 312.83 秒完成，全部 doc tests 通过。ID/arena 专项现约 45%；下一切片原子迁移 value/local/type resolution query products 与 compiler owner wiring，完成后再关闭 ID-1并进入 ID-2。

进展（2026-07-20）：ID-1h 已迁移并关闭 source/syntax identity 阶段。`ValueResolution` 的 name、qualified value、builtin associated value、variant enum 与 qualified type prefix 五张表，`LocalResolution` 的 definition/use 两张表，以及 `TypeResolution` 的 const-generic name 表全部从 cache-owned `HashMap<VersionedNodeKey, V>` 冻结为只读 `NodeMap<V>`；两张以稳定 `NodeSite` 为 key 的 type-name map 按设计保留。resolver 内部仍使用短生命周期 locator map，独立 API 创建隔离 owner，compiler 专用 value/type options 与 local origin capability 则统一绑定 `CompilerContext::node_store()`，没有为了 owner 增加 `ModuleOriginsQuery` 依赖。`ValueResolution` 的 consuming merge 通过 `NodeMapBuilder::extend_map` 在 signature const 合并时保留同 owner handle；local/value 测试修改改走显式 consuming builder，cache product 不再暴露写入口。closure audit 确认剩余 `HashMap<VersionedNodeKey, ...>` 只位于 canonical `NodeStore.by_locator`、resolver/checker 的可变执行状态和显式 semantic builders；回归锁定八张 resolution map 共用 compiler session owner以及 `NodeMap` contains/remove 行为。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-node-id 14、value/local/type resolver 3/12/7、body-check 139、compiler-query 118、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 319.52 秒完成，全部 doc tests 通过。ID-1 acceptance 至此完成，ID/arena 专项现约 50%；下一切片进入 ID-2，先量化 allocator 外 `ModuleId` 构造、graph/full-node clone 与 stable/local identity 边界，再按 graph snapshot owner 分批迁移。

进展（2026-07-20）：ID-2a 已删除 compiler graph lookup 的 full-node 身份双轨。审计确认全仓仍有 723 处 `ModuleId(...)` 直接构造，立即私有化 tuple 字段会把首切片退化为高风险机械改写；同时 `ModuleGraphLookup::module` 没有真实消费者，完整 `ModuleNode` 只被 trait 默认 path/parent/child lookup 间接拉取。`ModuleNodeRef::{Borrowed, Shared}`、`ModuleGraphNodeQuery -> Option<Arc<ModuleNode>>`、每次 query miss 的 node clone/Arc allocation，以及对应 `changed_graph_modules` diff/invalidation 现已整体删除；lookup trait 只暴露 entry、package root、path、parent 与 child declaration 精确 facts，`ModuleGraph`、`Arc<GraphLookup>` 和 compiler query adapter 分别实现同一契约。declarative compiler registry 从 116 收敛到 115 项，回归明确拒绝 `module_graph_node` 并锁定五类精确 graph fact query。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 118、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 313.19 秒完成，全部 doc tests 通过。ID/arena 专项现约 52%；下一切片为 graph allocator 引入 stable module key/local handle 映射，再把 loader、compiler input 与 checked/codegen DTO 的 owned `ModuleGraph` clone 收敛为共享 immutable snapshot，之后才私有化 `ModuleId` 构造。

进展（2026-07-20）：ID-2b 已建立 module stable/local identity 层并统一 graph snapshot 所有权。`StableModuleKey` 以 canonical `SourceIdentity` 为跨 session/stable 边界，并以共享 payload 避免节点与反向索引重复保存路径字符串；每个 `ModuleNode` 显式记录 stable key，`ModuleGraph` 通过 `by_stable_key` 提供 stable key ↔ local `ModuleId` 双向映射，原 source-identity API 只作为稳定边界 adapter。mutable graph 仅存在于 loader 构建/增量扩展阶段；冻结后由 8-byte `ModuleGraphSnapshot` handle 贯穿 loader query、`LoadedProgram`、compiler input、`ModuleGraphQuery`、`CheckedProgram` 与 `CodegenProgram`，不再在每层深 clone modules、path indexes、package roots、diagnostics 和 symbol resolver。snapshot 继续实现精确 `ModuleGraphLookup`，不会恢复已删除的 full-node query；provider graph growth 测试在显式 thaw-copy/freeze 边界修改 snapshot。回归锁定 stable/local 往返、stable key 与 snapshot 均为 8 bytes，以及 input/cache/checked/codegen graph allocation 全链 `ptr_eq`。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 38、compiler-query 119、driver 484、LLVM 177 项测试通过，CLI commands 50 项自然并发 311.74 秒完成，全部 doc tests 通过。ID/arena 专项现约 56%；下一切片按 production、fixture、public API 三层审计 723 处直接 `ModuleId(...)` 构造，先让 production 只从 graph allocator/stable lookup 获得 local handle，再决定 owner id/generation 的兼容迁移方式。

进展（2026-07-20）：ID-2c 已完成 production allocator 边界审计。`nia-ids` 新增显式 `ModuleIdAllocator` 与 `ModuleId::index()` 读取边界，`ModuleGraph` 持有 allocator 生成 entry/child local handle；loader graph query 不再按裸 dense index 重建 `ModuleId`，而是遍历真实 `ModuleNode` 身份。mangle、driver、monomorphize、program-signatures、codegen 与 graph storage 的 production 读取统一通过 `index()`；排除独立测试文件并在 inline test module 边界截断后，production 中只剩 `ModuleId` 定义与 allocator 内部的合法构造，测试 fixture 与公开 tuple 字段暂不机械改写。新增 allocator dense-index 回归和 graph fixture stable-key 回归。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-ids 1、nia-imports 1、loader-query 38 项定向测试通过，CLI commands 50 项自然并发 308.24 秒完成，全部 doc tests 通过。ID/arena 专项现约 58%；下一切片按 fixture 生命周期分组迁移裸 `ModuleId(...)`，再设计带 owner/generation 的兼容 opaque handle，不能在 owner 语义确定前直接私有化 tuple 字段。

进展（2026-07-20）：ID-2d 完成首批 semantic fixture 生命周期迁移。`nia-abi-check`、`nia-flow-check`、`nia-type-normalize`、`nia-type-resolve` 与 `nia-value-resolve` 的单模块测试不再直接构造 `ModuleId(0)`；每个测试或 pipeline helper 显式持有局部 `ModuleIdAllocator`，在同一生命周期内复用真实 `module_id`，没有新增全局或 `#[cfg(test)]` 魔法构造 API。上述五个 crate 的裸 `ModuleId(...)` 搜索已归零，为后续 owner/generation 加入 foreign/stale 检查保留清晰 fixture 边界。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；abi-check 2、flow-check 13、type-normalize 7、type-resolve 7、value-resolve 3 项定向测试通过，CLI commands 50 项自然并发 312.32 秒完成，全部 doc tests 通过。ID/arena 专项现约 59%；下一切片继续按单模块/多模块 fixture 分组迁移，优先清理共享 helper 中的裸构造，再进入 public API tuple 私有化前的 owner 设计。

进展（2026-07-20）：ID-2e 完成 item-signature 单模块 fixture 迁移。`nia-item-signatures` 的十个测试以及共享 `signatures_ok` helper 现在都从局部 `ModuleIdAllocator` 获取并复用同一 `module_id`，裸 `ModuleId(...)` 搜索归零；fixture 不通过全局 helper 隐藏 owner，保留后续 foreign/stale handle 回归的注入点。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；item-signatures 10 项定向测试通过，CLI commands 50 项自然并发 314.26 秒完成，全部 doc tests 通过。ID/arena 专项现约 60%；下一切片迁移 layout/type-lower 等仍含大量单模块 helper，再处理多模块 fixture 的显式 allocator 拓扑，之后才评估 `ModuleId` 公共 tuple 字段私有化。

进展（2026-07-20）：ID-2f 完成 defs/layout/type-lower 单模块 fixture 迁移。`nia-defs` 的 6 个 definition collection 测试、`nia-layout` 的 9 个 layout pipeline 测试与 `nia-type-lower` 的 12 个 lowering 测试现在各自持有局部 `ModuleIdAllocator`，`collect_ok`、`compute_test_const`、program-defs map、canonical append 与 active-tree lowering 全部复用同一 `module_id`；三个 crate 的裸 `ModuleId(...)` 搜索归零，未引入共享全局构造 helper。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；defs 6、layout 9、type-lower 12 项定向测试通过，CLI commands 50 项自然并发 313.74 秒完成，全部 doc tests 通过。ID/arena 专项现约 61%；下一切片迁移 local-resolve 等共享 helper，再开始多模块 fixture 的 allocator 拓扑与 owner 语义回归。

进展（2026-07-20）：ID-2g 完成 local-resolution 与相邻单模块 fixture 迁移。`nia-local-resolve` 的 12 个测试、`nia-static-check` 的共享 check pipeline、`nia-trait-solve` 的 canonical-store 回归与 `nia-mangle` 的 error-type 回归全部改用局部 `ModuleIdAllocator`；原先以 `ModuleId(99)` 表示“外部模块”的 local-shadowing 用例现在从同一 allocator 分配第二个真实 handle，明确表达同 fixture 内的模块拓扑。四个 crate 的裸 `ModuleId(...)` 搜索归零，没有引入 process-global fixture identity。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；local-resolve 12、static-check 7、trait-solve 1、mangle 2 项定向测试通过，CLI commands 50 项自然并发 309.79 秒完成，全部 doc tests 通过。ID/arena 专项现约 62%；下一切片处理 const/body/backend 等共享 pipeline fixture，再进入真正多模块 graph fixture 与 owner/generation 设计。

进展（2026-07-20）：ID-2h 完成 const/body checked pipeline fixture 迁移。`nia-const-check` 的 `CheckedFixture` 现在显式保存 allocator 产生的 `module_id`，const lowering、typed facts 与断言不再各自捏造入口 ID；`nia-body-check` 的 `TestBodyCheck` 同样携带真实身份并贯穿 type lowering、semantic uses、normalization、visible extension、trait impl 与 type-store append，两个 source-origin pipeline 使用各自局部 allocator。原先 field-shadowing 回归直接从 `defs.module_id` 引用当前模块。相邻 `nia-function-ir` shared store fixture 与 `nia-codegen-llvm` program-index fixture也完成迁移；上述测试文件的裸 `ModuleId(...)` 搜索归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；const-check 5、body-check 139、function-ir 5、LLVM 177 项定向测试通过，CLI commands 50 项自然并发 309.04 秒完成，全部 doc tests 通过。ID/arena 专项现约 64%；下一切片迁移 backend-lower/monomorphize 等剩余单模块 fixture，再处理 compiler-query 与真正多模块 graph fixture，随后进入 owner/generation 设计。

进展（2026-07-20）：ID-2i 完成 backend-lower fixture 身份迁移。共享 `TestBackendLowering` 现在保存局部 allocator 产生的 `module_id`，并将同一身份贯穿 defs/type/semantic/normalization/body/function/mono/program-const/backend input；`lowering.rs` 的完整 checked-program pipeline 同样只使用一个局部 ID。extension mutation closure 直接从 `defs.module_id` 取得 owner，generic local static 断言读取 fixture 身份；items 与 vtable cache 的独立测试各自持有 allocator。`nia-backend-lower/src` 的裸 `ModuleId(...)` 搜索至此归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；backend-lower 96 项定向测试通过，CLI commands 50 项自然并发 309.72 秒完成，全部 doc tests 通过。ID/arena 专项现约 66%；下一切片迁移 monomorphize/function-lower/function-opt 等 IR fixture，随后集中处理 compiler-query 与 codegen 大型多模块 fixture。

进展（2026-07-20）：ID-2j 完成 function IR 与 monomorphization fixture 身份迁移。`nia-function-lower` 的共享 `TypeStore` 现在与 allocator 产生的 `module_id` 绑定在同一静态 fixture，局部 store 测试则各自持有局部 allocator；`nia-function-opt` 的共享类型 fixture及 effectful callee `GlobalDefId` 复用同一身份。`nia-monomorphize` 引入显式 `TestFixture { module_id, type_store, types }`，defs、type append、generic instantiation、const-expression identity 与 module input 不再分别捏造入口 ID；`mono_input` 直接读取 `defs.module_id`，静态 empty collector 同样返回与 store 绑定的真实 ID。三个 crate 的裸 `ModuleId(...)` 搜索归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；function-lower 49、function-opt 52、monomorphize 9 项定向测试通过，CLI commands 50 项自然并发 310.16 秒完成，LLVM 177、compiler-query 119、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 67%；下一切片迁移 codegen function-IR 的大型单模块 fixture，再集中处理 compiler-query 的多模块 graph fixture与 owner/generation 设计。

进展（2026-07-20）：ID-2k 完成 LLVM function-IR fixture 身份迁移。`nia-codegen-llvm/src/tests/function_ir.rs` 的十九个直接 backend-program 测试现在各自从局部 `ModuleIdAllocator` 获取唯一模块身份，并将它贯穿 type-store append、typed/function lowering、backend module、global definition、const-expression、function-instance参数模块与 vtable identity；六处共享 `single_module_program` 调用改为显式传入当前 `module_id`，helper 不再隐藏入口模块假设。该文件 84 处裸 `ModuleId(0)` 已全部清零，24 项 function-IR 定向测试通过。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；CLI commands 50 项自然并发 310.22 秒完成，LLVM 177、compiler-query 119、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 68%；下一切片开始迁移 compiler-query 的大型多模块 graph fixture，先抽取 allocator-backed 测试拓扑，随后以真实多 owner 场景确定 `ModuleId` owner/generation 与 tuple 字段私有化方案。

进展（2026-07-20）：ID-2l 建立 compiler-query 的首个 graph-first 多模块 fixture 边界。新增 `LoadedProgramFixture` 直接从真实 `ModuleGraph` 获取 entry/child/shallow-child ID，统一保存 graph 与 loaded modules，并从同一 snapshot 生成 reorder、provider growth 和 source-revision update 输入；`intern_child`/`intern_shallow_child` 现在返回 graph allocator 实际创建的 handle。输入 source-identity 索引、重复 ID/identity、列表重排、additive graph growth、provider cache 保留/失效、semantic provider activation、method provider 精确失效、revision-only update 与 type-store slot persistence 等二十项回归已迁移；旧 shallow-entry helper 删除，刻意测试 local-ID replacement 的用例也改由显式 allocator 产生两个身份。compiler-query 裸构造从 403 降至 339，首批清理 64 处，更重要的是已删除这批测试中“传入 index 后由另一 allocator 猜出同值”的身份双轨。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 310.02 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 70%；下一切片继续让单模块 query 与 program-set 回归使用 graph-first fixture，逐步淘汰 `loaded_program_with_modules`/`module_graph_for_loaded_modules` 的 dense-index 兼容路径，再基于真实 graph owner 测试确定 generation 与 tuple 字段私有化方案。

进展（2026-07-20）：ID-2m 将 compiler-query 的 canonical type-store 与增量 cache 回归接入 graph-first fixture。`LoadedProgramFixture::database` 统一从同一 graph snapshot 创建 compiler database；type normalization、const/body synthesized type publication、signature/full normalization顺序与跨 database type-store isolation 五项测试均读取各自 graph 的真实 entry handle，跨 database 用例不再假定两个 session 的 local ID 数值相等。function body、body-local type、function signature 与双模块 signature-program-context 更新五项回归通过 `update_module_source` 保留同一 graph/ID，只替换 source revision；helper module 也由 entry graph allocator 创建。compiler-query 裸构造从 339 降至 318，本切片清理 21 处；累计 graph-first 迁移已清理 85 处。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 308.68 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 71%；下一切片迁移 source-identity/path replacement 特殊 fixture与 module-set query 回归，继续缩小 dense-index helper 消费面，之后再删除兼容 helper并进入 owner/generation 实装。

进展（2026-07-20）：ID-2n 迁移 compiler-query 的 source replacement、foreign handle 与 module-set fixture。`LoadedProgramFixture::update_module_path` 在保留同一 graph snapshot/entry handle 的前提下显式替换 loaded-module path/identity，两项 source identity invalidation 回归不再通过重建 graph 偶然复用 index；query provider override 使用 graph-first program，missing-module provider 则从独立 allocator 产生确定的 foreign handle，诊断断言动态引用该身份，不再捏造 `ModuleId(99)`。跨模块 body signature/trait-method index 两项回归直接使用 graph child handle；program signature item-set 的七模块拓扑与 extension-provider 的六模块拓扑均由 graph allocator 创建，全部期望集合引用实际返回的 module ID。compiler-query 裸构造从 318 降至 285，本切片清理 33 处；累计 graph-first 迁移已清理 118 处。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 308.64 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 72%；下一切片迁移 precise module-fact、layout、public-surface 与 batch product 回归，继续移除 `loaded_program_with_modules` 调用；其消费面足够小后删除 dense-index graph reconstruction helper，再进入 owner/generation 实装。

进展（2026-07-20）：ID-2o 完成 compiler-query precise product fixture 迁移。program type-alias signature、full layout、ABI signature index、public-surface snapshot、item-tree batch handle、executable value-ref、module-def node owner 与 extension semantic query 等单模块回归统一从 `LoadedProgramFixture` 读取 entry handle；所有 query key 与 `GlobalDefId` 不再重复构造入口 ID。跨模块 layout/signature-layout 两项回归由 graph allocator 创建 imported module，trace 断言动态匹配实际 entry/child handle，而不是假定 `ModuleId(0/1)` 文本；batch query 的单项与批量 key共享同一真实 handle。compiler-query 裸构造从 285 降至 240，本切片清理 45 处；累计 graph-first 迁移已清理 163 处。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 307.51 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 74%；下一切片迁移 extension/cache update 与 resolution/checked-product 回归，继续压缩 dense-index helper 消费面；随后删除兼容 helper并进入 owner/generation 实装。

进展（2026-07-20）：ID-2p 完成 compiler-query resolution 与 checked-product fixture 迁移。extension provider body-update cache 现在通过 `LoadedProgramFixture` 保持 graph/handle 并只替换 source revision；value resolution、flow/static/body check、const module、semantic-use 与 resolution node-owner 回归统一复用 graph entry handle，跨模块 plain-path resolution 由 graph allocator 创建真实 child。checked-module semantic product 以及 signature/body/function/static/normalization 等 cache-handle query key 不再重复捏造入口 ID。compiler-query 裸构造从 240 降至 196，本切片清理 44 处；累计 graph-first 迁移已清理 207 处。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 313.26 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 76%；下一切片集中迁移剩余 facade/cache-update 多模块拓扑，清空 `loaded_program_with_modules` 的 43 个消费点并删除 dense-index graph reconstruction helper，随后进入 owner/generation 实装与 `ModuleId` tuple 字段私有化。

进展（2026-07-20）：ID-2q 删除 compiler-query 的 dense-index graph reconstruction 兼容层。输入校验、const/mono/backend product、executable reachability、type-only owner、static initializer 与 invalidation 回归全部改为从 `LoadedProgramFixture` 或显式 graph snapshot 获取 handle；`loaded_program_with_modules`、`module_graph_for_loaded_modules` 与 `loaded_program_with_entry_child` 三个 helper 及全部消费点归零。stable source replacement 回归不再把 `ModuleId(1)` 塞进只有 entry 的伪 graph，而是建立包含真实 package-root handle 的第二张 graph、显式激活 semantic processing，再验证 stable identity 从旧 local handle 重映射到新 handle；这次迁移实际消除了 graph/modules 不一致的测试状态。compiler-query 裸构造从 196 降至 70，本切片清理 126 处；累计 graph-first 迁移已清理 333 处。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 312.35 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 80%；下一切片迁移最后 70 处复杂 facade/reexport/reachability graph 的手写身份与动态 trace 断言，随后进入 owner/generation 实装及 `ModuleId` tuple 字段私有化。

进展（2026-07-20）：ID-2r 完成 compiler-query 最后一批复杂 graph fixture 身份迁移。facade/private provider、reexport trait impl、forwarded const、incremental receiver type、generic trait reachability 与 cross-module backend/vtable 回归全部由 `LoadedProgramFixture` 的 graph allocator 创建 entry/child handle，trace 与 `GlobalDefId` 断言动态引用实际 owner；fixture 新增显式 child visibility 入口，不再通过手工 graph 与 loaded-module 两条路径分别表达 private provider。迁移同时修复了 executable facade 回归中 `args_impl`/`init_impl` graph child ID 与 loaded source 对调的旧状态，现在每个 child handle 对应其真实 path/source，既有语义回归仍通过。compiler-query 裸 `ModuleId(...)` 从 70 降至 0，ID-2l 至 ID-2r 累计清理全部 403 处手写构造。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 119 项定向测试通过，CLI commands 50 项自然并发 311.97 秒完成，LLVM 177、driver 484 项及全部 doc tests 通过。ID/arena 专项现约 84%；fixture/API 收口阶段完成，下一切片进入 `ModuleId` owner/generation 语义实装与 tuple 字段私有化，先审计跨 graph handle 混用的可检测边界，再决定 generation 是编码进 handle 还是由 graph owner token 验证。

进展（2026-07-20）：ID-3a 落地 owner-aware module handle 并私有化构造。`ModuleId` 从公开 32-bit tuple index 改为私有 `(owner, local index, generation)` 句柄：独立 allocator 的相同 local index 不相等；graph clone 保留已有 handle 与各自稠密 next index，但分支在同一 slot 新建节点会获得不同 generation；`Debug` 继续只显示稳定 local index，避免并发全局 owner/generation 污染诊断与符号文本。`ModuleGraph::get/get_mut` 现在同时校验完整 handle，不再让 foreign `ModuleId(0)` 按 index 别名到本图 entry；所有直接 tuple 构造已迁移到 allocator 或 graph entry/child。第一版 clone 共享原子 next slot 会让 speculative loader graph 消耗主图 index 并触发 dense-vector 断言，最终模型据此改为 local counter + global generation，driver 484 项恢复通过。更严格身份还暴露并修复 program-signatures fixture 使用自造 symbol 配 `KnownSymbolText`，导致三个未知 child 路径与 slot 重合的问题；fixture 现使用真实 `SymbolTable`。`ModuleId` 尺寸由 4 bytes 增至 12 bytes，这是当前 correctness-first 选择，后续需以实际产品计数评估是否值得在不削弱 owner/generation 的前提下压缩。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-ids 3、imports 2、compiler-query 119、driver 484 项通过，CLI commands 50 项自然并发 320.01 秒完成，LLVM 177 与全部 doc tests 通过。ID/arena 专项现约 92%；下一切片审计剩余直接 `.index()` 容器访问是否都处于已验证 owner 边界，并补齐 graph fork/revision stale-handle 回归与 handle footprint 指标，之后关闭专项并回到 Phase D。

进展（2026-07-20）：ID-3b 完成 local-slot 边界审计并关闭 ID/arena 专项。`ModuleId::index()` 已重命名为语义明确的 `local_index()`；全仓使用点逐一分类后，只有 `ModuleGraph::get/get_mut` 与 append dense invariant 将其用于 Vec 寻址，前两者取 slot 后比较完整 owner/index/generation，append 则只接受本 allocator 刚创建的 handle。mangling、LLVM declaration、monomorphization、driver report、source-version 与 diff ordering 仅将 local index 用作同 graph 内稳定显示/排序，不承担身份判定。新增 graph fork 回归证明两个 clone 保留共同旧 handle、同 local slot 新节点 generation 不同且互相不可 lookup；snapshot 回归证明旧 revision 拒绝 fork 后新增 handle；foreign graph 同 index 拒绝回归继续保留。`ModuleId` 12-byte footprint 由 nia-ids 自动化守卫，暂不为压缩牺牲 owner/generation correctness。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-ids 3、imports 4、compiler-query 119、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 321.22 秒完成，全部 doc tests 通过。ID/arena 专项至此 100% 关闭；下一切片按 Phase D 从 `module_graph_state` 与 loader/compiler/driver update 协议审计开始，不再继续扩展 identity 兼容层。

### 阶段 D（P0）：统一模块/provider 依赖

1. 建立统一 revisioned fact graph；source hash、module existence、provider visibility、name existence 与普通 derived query 使用同一 node/runtime。
2. loader 与 compiler 共享 revision/dependency recorder，或成为一个 session DB 的不同 provider group。
3. 实现 stable fingerprint 与完整 red-green validation。
4. 把 driver provider-demand loop 转成 dependency-driven worklist。
5. 把跨 query/revision 的 reachability lifecycle接入 session scheduler 与 key dedup；单个 backend query 调用栈内的纯局部 foreign-ref traversal按 Phase F/G 的 per-item/CGU 粒度继续拆分，不形成第二份 fact graph。
6. 删除 `module_graph_state` 摘要式同步协议和 eager-clear 旧图。

Acceptance：一次冷 check 不再出现 driver 层重复 load/update round；provider 新增只注册 typed fact/provider，不修改 driver fixed point；green node 依赖 fingerprint 全部匹配；随机修改下 incremental 与 clean recomputation 等价。

进展（2026-07-20）：D-1 删除了 driver 的 `module_graph_state` 摘要式同步协议。旧协议把 loader graph 手工投影为 `(SourceIdentity, semantic_selected, process_used_paths, process_declared_children)` 元组列表，既复制 graph 真相又可能漏掉新字段；driver 现在持有上一轮 immutable `ModuleGraphSnapshot`，以完整结构相等直接判断 loader revision 是否改变，provider-demand fixed point 行为不变，也没有引入另一层 fingerprint adapter。`module_graph_state` 全仓搜索归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 38、compiler-query 119、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 337.19 秒完成，全部 doc tests 通过。Phase D 现约 8%；下一切片把 provider-demand 注册与随后的 graph loading 合并成 loader-owned typed update result，减少 driver 对注册、重载和稳定性判断的编排职责，为 dependency-driven worklist 留出单一更新边界。

进展（2026-07-20）：D-2 把 provider-demand 注册、graph invalidation、重载与 revision 比较收敛为 `LoaderDatabase::update_provider_demands` 的 typed 三态结果：`NoNewDemands`、`GraphUnchanged`、`GraphChanged(Box<LoadedProgram>)`。driver 和 LLVM test harness 不再自行维护 `add_provider_demands*`、旧 graph 保存或前后 snapshot 比较；graph 不变时 loader 只重算精确 `ModuleGraphQuery`，不会重建包含完整 module vector 的 `LoadedProgram`，只有 graph 增长才返回 boxed aggregate。回归锁定新增但不改变 graph、重复 demand 去重、graph growth 保持既有 module identity，以及稳定 graph 不执行 `loaded_program` query。旧 provider-demand API 在全仓归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 39、compiler-query 119、LLVM 177、driver 全量测试通过，CLI commands 50 项自然并发 318.44 秒完成，全部集成与 doc tests 通过。Phase D 现约 11%；下一切片将 typed update 接入 dependency-driven worklist，先把 provider demand 的新增 key 与 graph fact invalidation 建成可去重的 loader/compiler scheduler 输入。

进展（2026-07-20）：D-3 将 compiler-owned provider change 输入从重复 `Vec` 固化为 canonical `HashSet`。`CompileRequest` 仍接受任意 iterator，保持调用边界兼容；`CompilerInputs` 在接管请求时完成去重，`CompilerInputDiff` 直接复用该 set，不再每次 diff 从 vector 重建 scheduler/invalidation key 集。回归覆盖同一 demand 重复提交只保留一个 compiler input key。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 120、LLVM 177、driver 全量测试通过，CLI commands 50 项自然并发 315.01 秒完成，全部集成与 doc tests 通过。Phase D 现约 12%；下一切片用明确的 provider graph work item 替代 driver 内部 `(LoadedProgram, Vec<ProviderDemand>)` pending tuple，并让 finalization policy 消费 typed change set，而不是继续依赖 tuple 位置语义。

进展（2026-07-20）：D-4 将 driver 内部 `(LoadedProgram, Vec<ProviderDemand>)` pending tuple 提升为显式 `ProviderGraphWorkItem`，并把 `can_finalize_without_discovery` 策略封装为 work item 方法。provider graph update 的生产行为、discovery 顺序、compiler update 顺序均保持不变，但 tuple 位置语义和重复条件判断已删除，为后续 scheduler work item 扩展留下单一结构边界。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；compiler-query 120、LLVM 177、driver 全量测试通过，CLI commands 50 项自然并发 311.92 秒完成，全部集成与 doc tests 通过。Phase D 现约 13%；下一切片继续让该 work item 携带可去重的 dependency key，接入 loader/compiler dependency-driven scheduler，而不是再增加 driver fixed-point 分支。

进展（2026-07-21）：D-5 将 provider change 协议从 loader typed update 到 driver work item、`CompileRequest` 和 `CompilerInputs` 全程统一为 `HashSet<ProviderDemand>`。`ProviderDemand` 已具备 `Eq + Hash`，因此它在当前 compilation session 内直接作为 dependency key；新增 demand、finalization policy、compiler diff/invalidation 都消费同一集合语义，不再在层间退回顺序 `Vec` 或重复重建 set。`with_provider_changes` 仍接受任意 iterator，但 request boundary 立即去重；回归直接锁定重复 key 只保留一次。旧 `new_demands: Vec`、`provider_changes: Vec` 与 driver 显式 `Vec<ProviderDemand>` 协议在 crates 中归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 39、compiler-query 120、LLVM 177、driver 全量测试通过，CLI commands 50 项自然并发 310.47 秒完成，全部集成与 doc tests 通过。Phase D 现约 15%；下一切片审计 `ProviderRequest::ModuleSemantic { ModuleId }` 在 source invalidation/graph reset 后的 revision-local 身份，改为 stable module dependency key或显式重映射，避免把当前 session 去重误称为跨 revision stable fact graph。

进展（2026-07-21）：D-6 消除了 provider dependency key 中最后一个 revision-local module handle。`ProviderRequest::ModuleSemantic` 不再携带 `ModuleId`，而是记录目标 `SourcePath`；production body checker 通过 program context 的 module→source callback 生成该 key，loader 每次在当前 `ModuleGraph` 中按 normalized `SourceIdentity` 重映射后再标记 semantic selection。两个独立 graph owner 即使为同一模块分配不同 local handle，旧 demand 也会命中新 graph 的正确节点；回归显式构造不同 owner/handle 且初始未选中的 provider graph，验证 stable remap。旧 `ModuleSemantic { module_id }` 构造在 crates 中归零，compiler semantic-provider cache 与 driver discovery 顺序保持不变。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；body-check 139、loader-query 40、compiler-query 120、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 312.13 秒完成，全部集成与 doc tests 通过。Phase D 现约 18%；下一切片把 loader 的全局 `RwLock<HashSet<ProviderDemand>>` 与 blanket `ModuleGraphQuery` invalidation 收敛成 typed provider fact/worklist 输入，使新增 dependency key 能触发精确 graph work，而不是继续依赖共享可变集合。

进展（2026-07-21）：D-7 建立了 loader provider fact 到 module graph 的第一条正式 query dependency。raw `LoaderContext.provider_demands: Arc<RwLock<HashSet<_>>>` 被封装的 `ProviderFactStore` 取代，store 负责 canonical insertion/clear/snapshot；新增 cache-owned `ProviderDemandsQuery` 注册进 loader registry，`ModuleGraphQuery` 通过 `db.get` 读取该 fact。`update_provider_demands` 现在只 invalidates provider fact node，由 query runtime 沿 `module_graph → provider_demands` 依赖边传递失效，不再直接 `invalidate(ModuleGraphQuery)`；重复 key 不产生第三次 graph execution，稳定 graph 仍不重建 aggregate `LoadedProgram`。回归锁定 0-byte fact key、11 项 declarative registry、依赖边与精确 execution count；旧 raw demand lock 访问和直接 graph invalidation 在 loader 中归零。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 40、compiler-query 120、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 310.13 秒完成，全部集成与 doc tests 通过。Phase D 现约 22%；下一切片删除 `LoaderGraphState.applied_provider_demands` 这份重复 truth source，让 provider fact store 暴露 revision/delta，graph state 只记录 applied revision 并消费 typed worklist。

进展（2026-07-21）：D-8 将 loader provider facts 升级为 revisioned delta worklist。`ProviderFactStore` 现在保存单调 `ProviderFactRevision` 以及每个 canonical demand 的加入 revision；cache-owned `ProviderFacts` snapshot 可精确枚举全量 facts 或 `added_after(revision)`。`LoaderGraphState.applied_provider_demands` 整套集合副本已删除，只保留 8-byte applied revision；增量 graph update 仅消费新 revision 的 keys，source graph 重建则消费当前全量 facts。重复 key 不推进 revision，clear 在真实变化时推进 revision并清空 facts，均有自动化回归。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 120、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 311.64 秒完成，全部集成与 doc tests 通过。Phase D 现约 25%，整份 roadmap 加权约 37%；下一切片让 `ProviderDemandUpdate`、driver work item 与 `CompileRequest` 携带 provider fact revision，使 loader/compiler 开始共享 revision recorder，而不只共享 demand payload。

进展（2026-07-21）：D-9 将 provider fact revision 从 loader-local `u64` 升级为共享、owner-aware 的 `(store owner, index)` typed revision，并贯穿 `LoadedProgram`、`ProviderDemandUpdate`、driver `ProviderGraphWorkItem`、`CompileRequest` 与 `CompilerInputs`。revision 类型下沉到 provider 协议层并由 compiler-query 重导出，避免 loader↔compiler crate cycle；每个 `ProviderFactStore` 分配独立 owner，同 store 内 index 单调增长，不同 loader session 即使 index 相同也不相等。owner 分配与 revision index 均有 overflow guard，16-byte footprint、不同 store 隔离、loader update/program revision 一致以及 compiler 精确保留 request revision均有回归。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 121、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 313.05 秒完成，全部集成与 doc tests 通过。Phase D 现约 28%；下一切片把 revision 变化纳入 `CompilerInputDiff` validation，明确同 owner monotonic、owner replacement 与 provider change set 的合法组合，为 red-green validation 建立跨数据库事实版本守卫。

进展（2026-07-21）：D-10 为共享 provider revision 建立了 compiler update validation。`ProviderFactRevisionTransition` 明确区分 `Unchanged`、同 owner `Advanced`、跨 owner `Replaced` 与同 owner `Stale`；`CompilerInputDiff` 只接受 unchanged+空 change set、advanced+增量 change set，以及 advanced/replaced+空 change set 的 reset 语义，拒绝同 revision 伪增量、跨 owner 携带增量和 revision 回退。clear 或 loader session replacement 现在显式重置 executable fact session，不会误把旧 provider facts 当作同一事实世界；validation 以 typed error 在释放 compiler input 写锁后报告 ICE，非法请求不会写入 inputs 或 poison 数据库。既有 provider 增量回归全部改用真实单调 revision，新增回归覆盖 reset 分类、owner replacement、stale revision、同 revision change set 以及拒绝后 revision 保持可读。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 125、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 320.90 秒完成，全部集成与 doc tests 通过。Phase D 现约 32%，整份 roadmap 加权约 38%；下一切片把 provider fact revision 注册为正式 compiler input query 并建立 query dependency/invalidation，让事实版本变化进入统一 query graph，而不只停留在 update validation 与 out-of-band session reset。

进展（2026-07-21）：D-11 将 provider fact revision 接入 compiler query graph。新增 0-byte `ProviderFactRevisionQuery` 注册为第 116 项 declarative query contract，`CompilerDatabase::provider_fact_revision` 也统一经过该 input query；共享 `executable_check` 入口读取 revision，因此 `ExecutableProviderDemandsQuery` 与 `ExecutableCheckedModuleSetQuery` 都建立了正式依赖边。`CompilerInputDiff` 精确记录 revision 是否变化，仅在变化时 invalidates revision node，并由 runtime 传递失效两个 executable 产品；stable revision 的 timings-only update 保持它们 green。同步修正 executable checked-module store 的生命周期：不再每次 update 无条件清空，而只在对应 query 实际进入 invalidation closure 时清理，避免 green cached set ID 指向已删除 payload。回归覆盖 registry、两条 dependency edge、revision 传递失效、set 重算以及 stable revision 下同一 set ID 仍可 materialize。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 128、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 324.51 秒完成，全部集成与 doc tests 通过。Phase D 现约 35%，整份 roadmap 加权约 39%；下一切片把 revision-bound provider delta 注册为 cache-owned typed input/worklist query，让 executable incremental retention 从 query 读取版本匹配的 change set，继续删除 `invalidate_inputs` 对 provider worklist 的 out-of-band 操作。

进展（2026-07-21）：D-12 将 provider delta 固化为 revision-scoped、cache-owned 的 typed worklist。新增 0-byte `ProviderFactWorklistQuery` 作为第 117 项 compiler query contract，原 revision query 改为从同一个原子 `(revision, Arc<HashSet<ProviderDemand>>)` snapshot 派生；两个 executable 产品直接依赖 worklist，并由 revision 变化对 worklist root 做传递失效。`CompilerInputs` 在同 revision update 中共享同一 payload，在连续 advanced revision 尚未执行 query 时累积并去重 changes，advanced-empty reset 与 owner replacement 则原子清空，避免 latest-delta 覆盖造成丢 work。`ExecutableFactSession` 记录 applied revision 与已消费 key，只在 executable query 边界消费尚未应用的差集；provider-specific fact retention 已从 `CompilerDatabase::invalidate_inputs` 删除，graph body activation 暂保留独立路径。回归覆盖 query dependency chain、worklist/root 传递失效、多 revision 未消费累积、同 revision payload 保留、reset 清空、unowned/owned provider fact 淘汰以及 semantic provider 的自动消费 recorder。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 129、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 312.20 秒完成，全部集成与 doc tests 通过。Phase D 现约 39%，整份 roadmap 加权约 40%；下一切片把 `body_activated_modules` 提升为 companion graph worklist query，并由 executable session 在同一消费边界应用，随后删除 `invalidate_inputs` 对 executable fact session 的最后一条 graph-growth mutation 路径。

进展（2026-07-21）：D-13 将 graph body activation 接入第二条 cache-owned typed worklist。新增 0-byte `BodyActivationWorklistQuery` 作为第 118 项 compiler query contract；`CompilerInputs` 在同 graph/session 内累计 `process_used_paths: false→true` 的既有 module handles，连续 graph growth 尚未执行 query 时不丢 key，任何 executable fact full-reset 边界则清空 worklist。两个 executable 产品同时依赖 provider/body worklists，`ExecutableFactSession` 记录已消费 activation keys并只淘汰新激活 module 的旧 body/cache state。`CompilerInputDiff` 统一计算 `resets_executable_facts` 与 worklist 是否真实变化，仅在 payload 变化时 invalidates worklist root；旧 `retain_after_graph_growth` 与 `CompilerContext::retain_executable_facts_after_graph_growth` 已全仓删除，`invalidate_inputs` 不再执行任何 graph-growth retention。回归覆盖 registry、两条产品到双 worklist 的 dependency edges、连续两轮 activation 累积、worklist 传递失效与 session consumption recorder。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 130、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 310.12 秒完成，全部集成与 doc tests 通过。Phase D 现约 43%，整份 roadmap 加权约 41%；下一切片引入 typed executable fact epoch/reset query，让 target/runtime/root/source/provider-reset 也在 executable query 消费边界切换 session，删除 `invalidate_inputs` 对 fact session 的最后一条直接 mutation，使 compiler update 收敛为纯 input swap + query invalidation。

进展（2026-07-21）：D-14 以 typed `ExecutableFactEpoch` 关闭 compiler update 对 executable fact session 的直接 mutation。新增 0-byte `ExecutableFactEpochQuery` 作为第 119 项 query contract；compiler-local 8-byte epoch 只在 source/module executable inputs、provider reset、target/runtime 或 executable roots 变化时 checked-increment，普通 graph growth 与 timings update 保持原值。两个 executable 产品依赖 epoch input；`ExecutableFactSession::enter_epoch` 在 query 消费边界比较 recorder，不同 epoch 才原子替换整个旧 session，随后再消费 body/provider worklists。旧 `clear_executable_fact_session` 已全仓删除，`CompilerDatabase::update/invalidate_inputs` 现在只做 validated input swap 与 query invalidation，不再直接修改 executable fact payload。回归锁定 epoch/query footprint、双产品 dependency、reset 传递失效、update 后旧 session 暂存以及首次 query 时 epoch 切换并清除 sentinel facts。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；loader-query 41、compiler-query 131、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 311.38 秒完成，全部集成与 doc tests 通过。Phase D 现约 47%，整份 roadmap 加权约 42%；下一切片进入 stable fingerprint/red-green validation，先为 compiler input facts、worklists 与 epoch 建立确定性 fingerprint contract，再让 update 对内容相同的 input replacement 保持 green，而不是仅依赖手写字段 diff。

进展（2026-07-21）：D-15 建立 stable value fingerprint 的第一条内核运行路径。`nia-query` 新增 16-byte typed `QueryFingerprint`、domain-separated deterministic builder 与 declarative `StableValue` policy；cache slot 在 provider 执行后保存 value fingerprint，`validate_input` 比较 replacement fingerprint，相同则保留 root、dependency edges 与全部 dependents，变化才沿既有 reverse graph 染红。compiler 的 provider worklist、body activation worklist 与 executable epoch 成为首批三项 stable input contract，`invalidate_inputs` 删除对应 revision/worklist 手写布尔分支并统一走 red/green validation；provider `HashSet` 先逐项 canonical fingerprint 后排序，body activation 则从 revision-local `ModuleId` recorder 改为 `StableModuleKey -> ModuleId` worklist并只编码 stable source identity，避免无序迭代和 owner/generation 污染指纹。回归覆盖 fingerprint 16-byte footprint/domain separation、registry policy、无序集合插入顺序独立、内容相同 input replacement 零失效且 executable 产品零重执行、provider payload 变化精准失效而不误伤 body worklist。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 30、loader-query 41、compiler-query 132、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 321.48 秒完成，全部集成与 doc tests 通过。Phase D 现约 52%，整份 roadmap 加权约 43%；下一切片把 derived slot 从 eager clear 扩展为 potentially-outdated 状态，记录 dependency/value fingerprints并在 demand 时验证，先让少数稳定 derived facts 在上游 red 但结果未变时重新转 green，不把本切片的 input-root validation 虚报为完整 red-green。

进展（2026-07-21）：D-16 建立 derived query 的 demand-time red/green 状态机。`QueryState` 新增 `PotentiallyOutdated` 与互斥 `Validating`：stable input fingerprint 变红时只清空 input root，已有 stable dependents 保留旧 `Arc<Value>`、value fingerprint、dependency fingerprints 与 graph edges，非 stable dependents仍沿原 eager-clear 路径；首次 demand 通过 slot table 的 type-erased ensure callback递归拉起旧 dependencies，全部 fingerprint 匹配则原子恢复 `Ready` 并复用旧 handle，任何依赖变化、缺少 fingerprint、执行失败或验证期间再次 invalidation 才回落 provider 重算。query trace 新增 validation/green-validation counters；recorder 继续让所有普通 query 使用原 `HashSet<QueryNodeId>`，只有 declarative `StableValue` query 才额外分配 dependency fingerprint map，避免给当前 115 项非 stable compiler contracts增加常驻 edge payload。`ProviderFactRevisionQuery` 成为首个 production stable derived fact；模型回归证明 input `7→9` 时 parity 自身重算但同 fingerprint 让下游零执行且复用同一 `Arc`，`7→8` 时才继续染红并重算下游，production 回归同时锁定 provider revision validation counter。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 32、loader-query 41、compiler-query 132、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 312.47 秒完成，全部集成与 doc tests 通过。Phase D 现约 59%，整份 roadmap 加权约 45%；下一切片先补 validation/invalidation 竞态、连续 input revisions 与 stable `get_many` dependency recorder 的模型回归，再将 stable fingerprints扩展到 source/module identity、existence 和 eligibility facts，逐步缩小 eager-clear 子图。

进展（2026-07-21）：D-17 固化 red-green validation 的并发与连续 revision 语义，并扩展首批稳定 module facts。模型回归证明 validation provider 执行期间的第二次 invalidation 不会把旧值错误恢复为 green，连续两次 input revision 始终相对最新值验证，stable `get_many` 会为每个 child 记录 dependency fingerprint；`nia-query` 增至 35 项测试。compiler 的 `ModuleGraphPathQuery`、`ExtensionProviderSummaryQuery` 与 `ExtensionProviderModuleEligibilityQuery` 现声明 `StableValue`，fingerprint 只编码 package/path symbol、provider type semantic fields 及保持 equality 语义的 provider/method/value 顺序，不包含 revision-local `ModuleId`、地址或 debug 文本；`ProviderSummary` 只新增只读 provider slice 边界。graph path 与仍存在的 provider summary update 已从 eager invalidate 改为 `validate_input`，缺失/删除 module 仍保留 eager invalidation；production 回归证明 summary 内容变化会验证并精确重算稳定 eligibility 链。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 35、loader-query 41、compiler-query 133、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 311.36 秒完成，全部集成与 doc tests 通过。Phase D 现约 64%，整份 roadmap 加权约 47%；下一切片以 stable semantic identity 覆盖 graph parent/child/package-root，再推进 program/extension eligibility 链，继续缩小 eager-clear 子图。

进展（2026-07-21）：D-18 将 graph entry/parent/child/package-root 的缓存值从 revision-local `ModuleId` 改为 `StableModuleKey`，完成 compiler module graph 精确关系的稳定身份层。四类 query 均声明 `StableValue`，fingerprint 只编码 normalized source identity，child 额外编码显式 visibility discriminant；update 对这些 roots 统一使用 `validate_input`，不再 eager clear。`QueryModuleGraphLookup` 在消费边界把缓存的 stable key 映射到当前 graph handle，因此 green cache 不会保留并泄露旧 owner/generation 的 local ID；不存在或已删除关系以 `None` 稳定编码。graph owner replacement 回归证明 entry query 复用同一 `Arc` 但返回新 owner handle，graph fork 回归证明同一 stable child/package root 在不同 generation 间切换时关系 query 同样复用而 lookup 返回新分支 handle。严格 workspace/all-targets/all-features Clippy 无 warning，无参数的 `cargo test --workspace` 全部通过；nia-query 35、loader-query 41、compiler-query 135、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 309.23 秒完成，全部集成与 doc tests 通过。Phase D 现约 68%，整份 roadmap 加权约 48%；下一切片推进 parse-ok/semantic/program-signature/extension eligibility 的稳定集合指纹，继续缩小 graph change 后的 eager-clear fanout。

进展（2026-07-21）：D-19 将稳定验证从专用 input replacement 扩展到所有 query root invalidation，并迁移首批 program membership 链。`QueryDb::invalidate` 现在仍立即清空 root，但对具有 value fingerprint 的 transitive dependents保留旧 value/dependency fingerprints并标记 potentially-outdated；没有 fingerprint 的节点继续 eager clear，统一复用已有 validation race 状态机。模型回归证明普通 invalidate 后 derived parity 可重算、上层 stable parent 可 green 并复用原 `Arc`；`nia-query` 增至 36 项。compiler 新增只保存 `Vec<StableModuleKey>` 的 `StableModuleSequence`，parse-ok、semantic、program-signature module IDs 与 extension-provider module IDs 均声明 stable fingerprint，program-signature eligibility 也成为稳定 bool fact；任何需要 local IDs 的 checked/executable/signature/extension consumer 都同时读取当前 `ModuleGraphQuery` 做显式 remap，不把旧 owner/generation 写入稳定 cache。extension membership 直接依赖 parse-ok 与 per-module eligibility，不再被非稳定 discovery aggregate 阻断 green validation；三项 production 回归分别锁定 parse-error 内容变化、signature membership 不变和 provider-summary 内容变化时上层集合只 validation、不重执行并复用原 handle。严格 workspace/all-targets/all-features Clippy 无 warning；第一次原样全测因 6482 个历史 `nia_cli_test_*` 目录填满 `/tmp` 而在 linker 报错，清理后同一 `cargo test --workspace` 从头执行并以退出码 0 完成，nia-query 36、loader-query 41、compiler-query 137、LLVM 177、driver 484 项通过，CLI commands 50 项自然并发 334.27 秒完成，全部集成与 doc tests 通过。Phase D 现约 74%，整份 roadmap 加权约 50%；下一切片把 loaded source/module existence 与 public/using module relation纳入 stable fact layer，再收敛跨 loader/compiler 的重复 graph truth source。

进展（2026-07-21）：D-20 将 loaded membership、module path 与 public/using module relation 纳入 stable fact layer。`LoadedModulesQuery` 现在缓存按 `CompilerInputModule.source_identity` 构造的 `StableModuleSequence`，而不是 graph handle 或 graph stable key；source identity 未变但 graph owner/generation 替换时复用原产品并在当前 compiler inputs 中重映射 local handle，真实 identity 或顺序变化才染红。`ModulePathQuery` 以 normalized `SourcePath` 建立确定性 fingerprint；`PublicSurfaceModuleQuery` 与 `UsingScopeModuleQuery` 缓存 `Option<StableModuleKey>`，lookup 边界再映射当前 graph handle。对应 update 路径均从 eager invalidate 收敛为 `validate_input`，需要 local IDs 的 consumer 继续显式读取 `ModuleGraphQuery` 作为 mapping epoch 依赖。回归覆盖 loaded reorder、source identity 变化、同 identity owner replacement，以及 graph fork 后 child generation 变化时 graph/public/using relation cache 保持 green 而 lookup 返回新分支 handle。严格 workspace/all-targets/all-features Clippy 无 warning，compiler-query 137 项与无参数 `cargo test --workspace` 全部通过，CLI commands 50 项自然并发 310.10 秒完成，全部集成与 doc tests 通过。Phase D 现约 78%，整份 roadmap 加权约 51%；下一切片把 source version/module existence 纳入稳定事实，并审计 loader/compiler 重复 graph truth source 的收敛边界。

进展（2026-07-21）：D-21 将 `ModuleSourceVersionQuery` 纳入 stable input validation，同时明确禁止把 revision-local source handle 伪装成跨 session identity。fingerprint 确定性编码完整 `(SourceId, SourceRevision)`；只要 local `SourceId` 或 revision 任一变化，query root 和依赖它的当前产品就必须染红并替换，缺失 module 仍走 invalid-input/eager invalidation。update 路径从无条件 invalidate 改为 `validate_input`，为后续稳定 derived source facts提供统一 red-green 边界；新增回归专门构造“revision 数字相同但 `SourceId` 被替换”的输入，证明旧 `Arc<SourceVersion>` 不会复用。roadmap 同时把“无历史兼容包袱”提升为开篇核心执行准则：允许并发读者持有隔离的 immutable old revision，但旧 local handle、fallback、重复 truth source 和双轨 API 在 validation 后不得从 current revision 可达。严格 workspace/all-targets/all-features Clippy 无 warning，compiler-query 138 项与无参数 `cargo test --workspace` 全部通过，driver 484、LLVM 177 项通过，CLI commands 50 项自然并发 326.20 秒完成，全部集成与 doc tests 通过。Phase D 现约 80%，整份 roadmap 加权仍约 51%；下一切片把 loader 的 source existence/version map 从 `LoaderGraphState` 重复状态拆成正式 query facts，再据此收敛 loader/compiler graph truth source。

进展（2026-07-21）：D-22 将 loader source existence/version 纳入正式稳定事实，并删除 graph state 的重复真相与 source update cache mutation。新增 4-byte `SourceStatusQuery(SourceId)`，值明确区分 `Missing` 与 `Present(SourceVersion)`，fingerprint 编码 local source handle、存在性和 revision；parse/declaration key 构造与 `ModuleGraphQuery` 由此建立正式依赖，missing→present 和 revision change 都进入同一 red-green 路径。`LoaderGraphState.source_versions`、`graph_source_versions` 整张 map 和 `reset_graph_state` 已全仓归零；graph query 在 provider facts 没有可安全追加的纯 delta 时丢弃旧 seed 并重建。`ProviderFacts` 新增 reset revision，使 clear 后即使又加入 demand 也不会错误沿用带旧 provider 标记的 graph；只有同 owner、未跨 reset 的 advanced revision 可增量扩展。`LoaderDatabase::set_source/invalidate_source` 现在只修改 source/provider inputs 并 invalidates typed roots，不再直接清 graph work product，旧 graph 只在 query 消费边界被替换。回归覆盖 missing→present、source revision graph owner replacement、update 后旧 graph 暂存至首次 query、source-status dependency/registry，以及 provider additive/reset 两种 revision 语义。严格 workspace/all-targets/all-features Clippy 无 warning，loader-query 44 项与无参数 `cargo test --workspace` 全部通过，driver 484、LLVM 177 项通过，CLI commands 50 项自然并发 325.79 秒完成，全部集成与 doc tests 通过。Phase D 现约 84%，整份 roadmap 加权约 52%；下一切片审计 `LoaderGraphState { graph, applied_provider_revision }` 的剩余 work-product ownership，并形成 loader/compiler 单一 session DB 的原子迁移切片，不能再增加跨 DB 摘要或兼容 adapter。

进展（2026-07-21）：D-23 删除了 loader 最后一份 out-of-band graph work-product state。`ProviderFactStore` 现在 append-only 保存 owner-aware revision 事件：`Root`、携带精确新增 key set 和 predecessor 的 `Added`、以及截断旧 provider lineage 的 `Reset`；current `ProviderFacts` snapshot 仍只暴露当前 canonical demand set，不复制事件历史。新增 16-byte `ModuleGraphRevisionQuery(ProviderFactRevision)`：root/reset revision 从 source/declaration facts重建，additive revision 读取上一 revision graph、保留既有 module handles并只应用本轮 demands；0-byte `ModuleGraphQuery` 仅依赖当前 provider snapshot并选择对应 revision product。source change 会 invalidates 全部实际 source dependencies，reset revision 不再引用旧 provider graph；provider growth 则通过 query dependency chain复用上一 immutable snapshot。`LoaderGraphState`、`graph_state` 和 `applied_provider_revision` 已在 loader 全部归零，context/update API 不再拥有 graph cache。回归锁定 13 项 registry、revision key footprint、自依赖 revision chain、additive graph identity 保持、stable graph 不重建 aggregate，以及 source reset 后 provider-only module 从新 graph 消失。严格 workspace/all-targets/all-features Clippy 无 warning，loader-query 44 项与无参数 `cargo test --workspace` 全部通过，driver 484、LLVM 177 项通过，CLI commands 50 项自然并发 315.24 秒完成，全部集成与 doc tests 通过。Phase D 现约 87%，整份 roadmap 加权约 53%；下一切片按 crate dependency、context ownership 与 update transaction 审计 loader/compiler 单一 session DB 的原子迁移路径，再推进 driver provider lifecycle 和 reachability worklist，不能引入新的跨 DB adapter。

进展（2026-07-21）：D-24 建立了 loader/compiler 的单一 query session runtime。`nia-query` 新增 `QuerySession`，session 统一拥有 owner identity、跨 typed-DB database registry 和唯一 dependency graph；各 `QueryDb<C>` 继续拥有类型安全 context/cache/slot provider group，但 node frame、slot、ensure、fingerprint、trace、dependents 与 invalidation 都通过 session resolver跨组工作。thread-local recorder 以 session identity 决定是否记录跨 DB edge，不同 session 嵌套调用保持隔离；`QueryNodeId` 仍是 8-byte `(global db id, local index)`，session identity 位于执行 stack frame，没有增加每条依赖边的常驻 footprint。模型回归证明 shared-session stable child input 会跨 DB invalidates parent、递归 ensure/fingerprint 并重算，separate sessions 不记录 edge；原并发、cycle、连续 revision 与 `get_many` 模型全部保持通过。loader/compiler 分别提供显式 in-session constructor，Driver production 创建 compiler 时加入 loader session；loader key/owner 替换时旧 compiler 不再跨 session update，而是原子创建属于新 loader session 的 compiler。回归覆盖首次 session 配对和 loader replacement 后重新配对。严格 workspace/all-targets/all-features Clippy 无 warning，nia-query 38、driver 485 项与无参数 `cargo test --workspace` 全部通过，loader 44、compiler-query 138、LLVM 177 项通过，CLI commands 50 项自然并发 316.32 秒完成，全部集成与 doc tests 通过。Phase D 现约 90%，整份 roadmap 加权约 54%；下一切片让 compiler input roots 在 shared session 内直接读取 loader facts，删除 driver 的 `LoadedProgram → CompileRequest` 手工同步和 pending DTO protocol，之后再关闭 provider lifecycle/reachability scheduler 的剩余边界。

进展（2026-07-21）：D-25 建立首批 production loader→compiler fact edges，并删除 Driver 的 loaded-program 同步协议。`nia-compiler-query` 新增唯一 `LoaderFactProvider`；`CompileRequest::new` 对 standalone `LoadedProgram` 与 production `LoaderDatabase` 都走同一 trait 入口，没有增加 from-loader/from-program 双构造 API。compiler `ModuleGraphQuery`、`LoadedModulesQuery` 与 `ProgramLoadDiagnosticsQuery` 在自身 query frame 内分别读取 loader graph、stable source membership 和 diagnostics，因此 shared `QuerySession` 会记录 `loaded_modules → loader module_graph` 与 `program_load_diagnostics → loader load_diagnostics` 等真实跨 DB edge。Driver 不再调用 `load_program`、不再携带 pending program/revision，`ProviderDemandUpdate::GraphChanged` 也删除 boxed program；provider loop 只传递真正属于 compiler-local update 的 demand delta。新增回归从同一 session 构造两类 DB 并锁定跨 DB dependencies；严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过，loader-query 45、compiler-query 138、driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 312.42 秒完成，全部 emit-exe 集成与 doc tests 通过。Phase D 现约 93%，整份 roadmap 加权约 55%。当前 `CompilerInputs::new` 仍会通过 provider materialize aggregate `LoadedProgram`，供尚未迁移的 module fields、public/using snapshots、target/runtime/symbol 与 update diff 使用；这是下一迁移边界而非允许长期保留的 adapter。下一切片应让 per-module/source roots直接读取 loader facts，逐项删除 aggregate 字段和手工 invalidation，最终删除 compiler 对 loaded snapshot 的依赖。

进展（2026-07-21）：D-26 将 stable per-module facts 与 immutable loader configuration继续迁入唯一 provider。compiler `ModulePathQuery`、`ModuleSourceVersionQuery` 和 `ExtensionProviderSummaryQuery` 不再从 `CompilerInputs.modules` 读取，production provider 分别调用 loader graph、`SourceStatusQuery` 与 `ProviderSummaryQuery`，shared session 回归锁定 `module_path → loader module_graph`、`module_source_version → loader source_status` 和 `extension_provider_summary → loader provider_summary` 三条跨 DB edge。所有 compiler symbol consumers 改为从 loader-owned `SymbolTable` 读取，`CompilerInputs.symbols` 已删除；target/runtime roots也只从同一 provider 获取，不再读取 compiler snapshot。迁移审计同时确认不能把 item-tree/origin/parse roots机械接到 aggregate `LoadedModuleQuery`：该 aggregate 没有 stable fingerprint，任意 source invalidation 会把无 fingerprint compiler roots及其 dependents扩大清除，倒退 D-19/D-20 的精确性；因此这些字段必须先在 loader 拆成可独立验证的 field facts，再原子删除 compiler snapshot路径，而不是保留双轨或接受清图退化。严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过，loader-query 45、compiler-query 138、driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 322.73 秒完成，全部 emit-exe 集成与 doc tests 通过。Phase D 现约 94%，整份 roadmap 加权仍约 55%；下一切片建立 loader field-level item-tree/origin/parse facts与 fingerprints，再迁移对应 compiler roots和 precise invalidation。

进展（2026-07-21）：D-27 建立 value-equality red-green policy，并完成其余 module field roots 的跨 DB 迁移。`nia-query` 新增 `QueryFingerprintPolicy::SemanticValue` 与 `QueryKey::values_equal`：transitive invalidation 只把旧 slot value保留到本次 validation，provider 重算后仅当新值可在所有当前消费者中完整替代旧值时复用 session-local fingerprint，否则推进 slot semantic revision；比较结束即释放 cache 对旧值的引用，不建立 revision history cache。这里的 equality 契约包含 local identity freshness，不能把“声明文本语义相同”误当作“含旧 `SourceRevision` handle 的物理 value 可替代”。loader 新增以稳定 `SourceId` 为 key 的唯一 raw item-tree fact、active/signature item-tree、origins 与 parse-errors field queries，compiler 对应十类 roots均通过唯一 `LoaderFactProvider` 读取并记录 shared-session edge；安全审计后已删除同值同策略的 raw `Definition/Declaration/Full` 三 kind，active fact也只保留真正不同的 `Full/Signature/ConstSignature`。所有携带 `VersionedNodeKey` 的 tree facts和 compiler roots采用完整 equality，source revision 变化必定刷新其 fingerprint 与全部 handle-bearing dependents。standalone `CompilerInputDiff` 同样在 source revision 变化时强制刷新 origins、raw/active/declaration/signature/full tree roots，原先要求跨 revision 保留 declaration/definition products 的测试契约已删除；stable public surface、using scope 与 provider summary等不携带 local handle 的独立投影仍可保持 green。回归同时持有旧 `Arc` 模拟真实历史读者，并证明 validation 后 current item tree 与 `DefCollection.def_nodes` 已更换且所有 key均属于最新 `SourceVersion`；旧 snapshot 只由外部读者存活，不再从 current query graph 可达。field key 尺寸锁定为 4/8 bytes，没有缓存 revision-local `ModuleId`。严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过，`nia-query` 39、loader-query 47、compiler-query 138、Driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 365.40 秒完成，全部 emit-exe 集成与 doc tests 通过。Phase D 现约 96%，整份 roadmap 加权约 56%；下一切片把 public/using surfaces、executable value-ref index 和 standalone diff从 `CompilerInputs` 迁成 query facts，最终删除 aggregate `LoadedProgram` snapshot。

进展（2026-07-21）：D-28 删除 compiler-owned public/using eager snapshots与其手写失效传播。`CompilerInputs` 不再保存 `Arc<PublicSurfacesQueryValue>` 或 `Arc<PublicUsingScopesQueryValue>`，`CompilerInputs::new` 也不再从为 executable index 暂留的 eager defs计算这两份 aggregate；`PublicSurfacesQuery` 现在显式依赖 `ParseOkModuleIdsQuery → ModuleDefsQuery` 与 `ModuleGraphQuery`，`PublicUsingScopesQuery` 显式依赖同一 defs/graph及 `PublicSurfacesQuery`。module 与 name projection全部改为读取这些 query products，不再通过 context 回到 input snapshot；aggregate、module surface/scope、value/type/unresolved projections均使用包含 graph-local identity freshness 的完整 equality red-green，module-name projection继续输出 `StableModuleKey` 并在 lookup边界映射当前 graph handle。`CompilerInputDiff` 中 public/using changed booleans、module/name set计算及 `invalidate/validate_input` 循环全部删除，净删约 300 行重复同步逻辑。依赖回归锁定 `public_surfaces → module_defs/module_graph`、`public_using_scopes → public_surfaces/module_defs` 与 module projection edges；body-only revision会验证重算 aggregate，但未变化的 module using projection保持同一 cache `Arc`，graph owner replacement则刷新携带 local `ModuleId` 的中间 value并保持 stable-key lookup语义。严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过，compiler-query 138、Driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 371.92 秒完成，全部 emit-exe 集成与 doc tests 通过。Phase D 现约 97%，整份 roadmap 加权约 57%；下一切片迁移 eager executable value-ref index并继续拆除 aggregate loaded module snapshot。

进展（2026-07-21）：D-29 删除 `CompilerInputs::new` 的 eager executable value-ref index与对应手写 owner diff。compiler update 不再为所有 loaded modules提前 collect defs、扫描 active item tree并保存 `GlobalDefId → item location` map；`ExecutableValueRefItemIndexQuery(ModuleId)` 只在 executable reachability真实需要扫描引用时，依赖 `FullActiveModuleItemTreeQuery + ModuleDefsQuery` 按模块构建一次 cache-owned index，`ExecutableValueRefItemQuery(GlobalDefId)` 再作为 owner projection读取它，避免逐 owner 线性扫描退化为 O(items × owners)。location value只保存 top-level item index 与当前 revision `VersionedNodeKey`，不再嵌入整棵 `Arc<ActiveModuleItemTree>`；consumer 显式读取 current full tree后构造单 item view。旧 `ExecutableValueRefItemLocation`、input map、context getter、`changed_executable_value_ref_items` 全量比较和 update invalidate loop全部删除。依赖回归锁定 owner projection → module index → full tree/defs，revision-only update证明 current owner node key刷新到最新 `SourceRevision`。严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过；compiler-query 139、Driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 368.66 秒完成，全部 emit-exe 集成与 doc tests 通过。Phase D 现约 98%，整份 roadmap 加权约 58%；下一切片删除 aggregate loaded module snapshot与剩余 standalone field diff。

进展（2026-07-21）：D-30 删除 compiler 最后一份 aggregate loader snapshot 与 standalone field diff。`CompilerInputs` 现只保存 provider fact revision/worklist、optimization 与 timing；graph、module vector/index、diagnostics、target/runtime、`CompilerInputModule`、`ChangedModuleInput`、`CompilerInputDiff` 及全部 module/graph/diagnostic 手写 invalidation 已归零。graph relation projections、executable roots、body activation worklist 与 executable epoch均从当前 tracked facts推导；后两者使用包含 local `ModuleId`/`SourceVersion` freshness 的完整 equality，provider advanced-empty reset也在 executable session消费边界清空旧事实。`LoaderFactProvider` 显式暴露自身 `QuerySession` 与 canonical `NodeStore`，`CompilerDatabase::new` 自动加入该 session，公开 `new_in_session` 与可替换 loader-fact `RwLock` 已删除；`update` 只接受同 session tracked provider，immutable `LoadedProgram` 仍可用于一次性 compilation，但不能伪装成可增量更新的 provider。Driver、LLVM 测试和 production loader都走唯一自动配对入口。compiler-query 的增量 fixture改为 test-only tracked fact DB，不再依赖 production snapshot diff，并保留 source freshness、stable projection green、provider worklist 与 executable cache回归。严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过；compiler-query 136、loader-query 47、Driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 495.11 秒完成，全部 emit-exe 集成与 doc tests 通过。Phase D 现约 99%，整份 roadmap 加权约 59%；下一切片收敛 provider lifecycle/reachability scheduler并关闭 Phase D。

进展（2026-07-21）：D-31 删除 loader→Driver→compiler 的 provider revision/delta 重复同步链。新增 canonical `ProviderFactSnapshot`，以 current revision、reset watermark 与去重后的 current demand set完整表达 loader-owned provider state；compiler `ProviderFactWorklistQuery` 直接在 shared `QuerySession` 中读取 loader `ProviderDemandsQuery`，正式记录跨 typed-DB dependency edge。`CompileRequest.provider_fact_revision/provider_changes`、两个 builder、`CompilerInputs` 对应字段、merge/transition validation与 Driver/LLVM 的 `compiler.update(...with_provider_changes)` 回灌全部删除；compiler update 现在只交换 optimization/timing。loader provider root采用 semantic red-green，reset watermark保证 compiler 即使跳过中间 clear revision，也会清空旧 executable facts后应用当前 demand set，而不是错误累加历史 key。测试 fixture 的 provider facts也拆成独立 tracked query，不再让 aggregate `LoadedProgram` revision污染 module fact equality。严格 workspace/all-targets/all-features Clippy 无 warning；compiler-query 133、loader-query 47、Driver 485、LLVM 177 项通过。Phase D 仍约 99%，整份 roadmap 加权约 59%；下一切片把剩余 Driver `ProviderGraphWorkItem/pending_update` fixed point整体迁入 compiler session 的 dependency-driven provider worklist入口，再审计 reachability session/store边界并关闭 Phase D。

进展（2026-07-21）：D-32 删除 Driver 最后的 provider fixed point。`CompilerDatabase::{check_program,entry_check_program,codegen_program}` 现在在 query root返回后向 loader canonical fact owner注册本轮 demands；graph growth通过 shared-session dependency invalidation推进 compiler-owned worklist，resolved-body-sensitive growth继续执行 executable discovery，纯 graph growth直接进入 final root。Driver `compile_with` 只建立/更新 session、调用一次目标入口并上报计数，不再包含 loop、pending DTO、graph update match或 provider semantic policy；LLVM test harness的复制循环也删除。loader 三态 `ProviderDemandUpdate` 收敛为只表达 graph 是否增长及是否影响 resolved body facts的 `ProviderGraphUpdate`，不再把 revision/delta重新暴露给编排层；immutable `LoadedProgram` 明确是单次 compilation fact provider，不伪装可变 loader。回归证明 compiler-owned worklist真实推进多轮，而 `entry_checked_program` 最终只执行一次；`ProviderDemandUpdate`、`ProviderGraphWorkItem`、`pending_update`与公开手工 executable-demand入口在 production 调用面归零。严格 workspace/all-targets/all-features Clippy 无 warning；compiler-query 133、loader-query 47、Driver 485、LLVM 177 项通过。Phase D 仍约 99%，整份 roadmap 加权约 59%；下一切片删除 reachability 的 `take/store` mutable session协议与 query 外 checked-module payload生命周期，再按 Acceptance 审计关闭 Phase D。

进展（2026-07-21）：D-33 关闭 Phase D。两个 executable product现在通过单一 session transaction串行消费同一 `ExecutableFactSession`：transaction开始时在 scheduler guard下取得状态，正常完成后无论 provider-demand还是 checked-modules分支都归还 applied revision/worklists；并发不同 query key不再同时取得空 session、相互覆盖，query panic则留下安全 default并允许后续 transaction恢复。`take_executable_fact_session`/`store_executable_fact_session` 已删除。`ExecutableCheckedModuleSetId`、context side store、materialize/clear API和 test-only `ExecutableCheckedModulesQuery`包装也全部删除；唯一 production `ExecutableCheckedModulesQuery` 直接以 cache-owned `Vec<Arc<CheckedModule>>` 保存 payload，query name不再保留旧 set身份。回归锁定并发 executable products 后 session revision仍存在、revision invalidation更换 cache allocation、稳定 input保持同一 allocation。新增确定性伪随机差分测试，以 24 轮 source revision在同一增量 session和每轮全新 clean session间比较完整 diagnostics及 module defs/body/semantic/provider摘要，全部等价。Acceptance审计同时确认 backend `ReachabilityWorklist` 只存在于单个 `BackendLoweringQuery`调用栈，不保存跨 revision state或第二份 dependency truth；其 per-item/CGU task化明确留在 Phase F/G，持久 worker/backpressure属于 Phase E。严格 workspace/all-targets/all-features Clippy 无 warning，无参数 `cargo test --workspace` 全部通过；compiler-query 135、loader-query 47、Driver 485、LLVM 177 项通过，CLI commands 50 项自然并发 484.51 秒完成，全部 emit-exe/integration与 doc tests通过。Phase D 至此 100%，整份 roadmap 加权约 60%；下一阶段进入 Phase E，先建立 session-owned persistent executor与资源预算，再删除 `get_many` 的临时 scoped worker。

进展（2026-07-21）：E-1 建立了 session-owned persistent query executor并删除 `get_many` 的临时 scoped worker。`QuerySession` 现在拥有惰性扩容、Drop shutdown/join 的 worker pool与单一活动任务预算；每个 batch 把 key-order result、父 query stack、dependency nodes/fingerprints和 panic payload作为 task result归并，calling thread在等待时帮助自己的 batch，因此单 worker与满预算嵌套 fan-out都能前进而不突破 session并发上限。helper禁止执行其他 batch 的任务，避免被帮助任务反向等待当前暂停 query slot形成跨 batch自锁；executor capacity与batch completion通过同一 condition-variable协议唤醒，单 worker下并发调用也不会丢唤醒。真实 `std.fs` workload在初版全局 helper策略下暴露该自锁，修正后默认并行度连续通过；确定性模型覆盖跨 batch反向依赖、满预算嵌套、peak-active上限、parent-stack cycle、panic恢复、invalidation-during-compute与non-Clone handle顺序。production审计确认 program-signature、checked-module、extension与reachability fan-out均只经`get_many`，query provider中不再存在第二个OS-thread入口。严格 workspace/all-targets/all-features Clippy无warning；无环境变量、无线程参数的`cargo test --workspace`最终完整通过，`nia-query` 43、compiler-query 135、loader-query 47、Driver 485、LLVM 177项通过，CLI commands 50项自然并发481.29秒完成，全部emit-exe/integration/doc tests通过。Phase E现约50%，整份roadmap加权约62%；下一切片把session executor预算接入Cargo jobserver/进程级协调并删除`NIA_QUERY_THREADS`旧覆盖，随后建立LLVM memory semaphore/backpressure。

进展（2026-07-22）：E-2 将所有 `QuerySession` 的活动任务接入唯一进程级 CPU 预算。runtime 通过正式 `jobserver` crate 一次性继承 Cargo/GNU Make jobserver；无可继承协议时才按 process-visible parallelism 建立本地 jobserver，并在两种路径中统一计入一个 implicit process token。session 仍拥有独立 queue/worker 生命周期，但 worker 和 calling-thread helper 执行 task 前都取得共享 RAII permit；同一线程上的跨 session nested batch 复用已有 permit，避免单-token预算自锁。helper delivery、pending request、无 waiter token 归还和 implicit token释放都由单一 mutex/condition-variable状态机管理，损坏的继承协议直接报错，不静默退回另一套调度真相。`NIA_QUERY_THREADS` 实现、文档入口与解析逻辑全部删除，没有保留旧覆盖；默认 session共享同一预算的 identity 守卫、两个独立 session 的 peak-active=2 模型，以及跨 session nested batch 的 peak-active=1 模型均已加入。严格 workspace/all-targets/all-features Clippy无warning；无环境变量、无线程参数的原样 `cargo test --workspace` 完整通过，`nia-query` 46、compiler-query 135、Driver 485、LLVM 177项通过，CLI commands 50项自然并发481.63秒完成，全部emit-exe/integration/doc tests通过。Phase E现约65%，整份roadmap加权约64%；下一切片建立 LLVM memory semaphore/backpressure，再删除 compiler API 内 `cfg(test)` resource permit。

进展（2026-07-22）：E-3 为 LLVM 重任务建立进程级 memory semaphore/backpressure。`nia-query::resources` 成为 effective system/cgroup memory limit与available pressure的唯一探测实现；原 `nia-test-support` 约200行重复 Linux/cgroup解析及对应第二份测试已删除，外层跨进程 session pool直接复用 production probe。全部 LLVM IR/object public入口在构建 `ProgramIndex`、LLVM context/module和输出bytes前取得同一个非`Send` RAII permit：capacity按 visible CPU与一半有效内存共同决定，每个重任务按1.5 GiB计费并最多并发4个；未知内存时串行，已有任务占用且当前余量低于2 GiB时阻塞生产者，而 active=0始终允许一个任务前进。thread-local identity depth使nested入口复用permit且乱序drop只在最后一个handle释放槽。timing新增`llvm.memory_permits`/`llvm.memory_waits`；确定性模型锁定capacity计算、两任务峰值、nested/乱序释放与cgroup父层最紧限制，CLI detail timing证明真实LLVM入口取得permit。`nia-query` 52项、LLVM 177项、test-support 8项、CLI定向回归与严格workspace/all-targets/all-features Clippy通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，CLI commands 50项自然并发496.94秒、process 39项126.36秒、Driver 485项84.45秒完成，全部emit-exe/integration/doc tests通过。Phase E现约80%，整份roadmap加权约66%；下一切片删除compiler API和LLVM test helper内的`cfg(test)` resource permit，仅由test harness编排外层session。

进展（2026-07-22）：E-4 删除了测试构建对compiler/LLVM public入口的隐藏资源语义并关闭Phase E。`CompilerDatabase::{check_program,entry_check_program,codegen_program}`内三处`#[cfg(test)] compiler_permit`与LLVM unit helper内四处重复permit全部删除，`nia-compiler-query`/`nia-codegen-llvm`对`nia-test-support`的dev-dependency同步归零；unit tests恢复libtest自然并发，只有CLI/Driver integration入口继续把完整进程/session声明给外层resource pool。compiler-query 135项在无permit下0.36秒通过，LLVM 177项由production memory semaphore控制并在4.95秒通过；严格workspace/all-targets/all-features Clippy无warning。无环境变量、无线程参数的原样`cargo test --workspace`完整通过，CLI commands 50项496.79秒、process 39项126.76秒、Driver 485项85.63秒、LLVM 177项5.00秒完成，全部emit-exe/integration/doc tests通过。Phase E五项实现与Acceptance全部满足，现为100%；整份roadmap因同时确认revision-slot retirement缺口加权约67%，下一切片先建立旧revision query slot的quiescent retirement与有界长寿命session模型，再进入Phase F。

### 阶段 E（P0/P1）：持久 executor 与测试资源模型

1. session 创建 executor，并通过 Cargo jobserver 或基于 visible parallelism 的同构本地 jobserver 共享进程级 CPU 预算。
2. `get_many` 与其他 fan-out改为提交 task，不临时创建 scoped worker。
3. body check、program signature fan-out 和 reachability scan 接入 executor。
4. LLVM 重任务加入 memory semaphore/backpressure。
5. test harness 只控制 session 数，移除 compiler API 内 `cfg(test)` permit。

Acceptance：无 `NIA_QUERY_THREADS` 也能稳定运行；普通 `cargo test` 不需 `--test-threads`；单编译和多测试并发都不超预算。

### C/D 后补充阶段（P0）：revision retirement 与有界 session

当前 query 的逻辑失效语义已经正确：input root变化会清除旧value，derived stable product只在dependency fingerprint一致时保持green，重算后的新value即使语义相等也会替换旧payload。然而typed cache按完整query key持有slot，slot identity table又append-only保留erased key；`ParsedModuleQuery(SourceVersion)`等revision-keyed节点因此可能在value失效后仍把旧local handle保留到session结束。cache本身不是query graph外已经取得snapshot的调用方，不能援引immutable snapshot例外。

实施顺序：先加入多轮source edit模型，分别观测typed cache、slot identity和dependency edge数量；再定义revision quiescence（无computing/validating节点且旧返回`Arc`只由外部持有）与retirement协议；可回收slot使用generation防止旧`QueryNodeId`误指新节点，或采用不复用index的tombstone并从typed lookup/identity/dependency graph删除旧key；最后让source/node revision owner在没有活跃快照时释放对应locator/payload。不能只清value后保留key，也不能用定期重建整个`QuerySession`逃避增量生命周期。

Acceptance：连续至少100轮body-only source edit后，旧`SourceVersion` query key/value与其forward/reverse edges在quiescence后不可从current cache/slot graph到达；slot/edge/live payload数量相对当前source/module/item规模有界，而不是随revision数线性增长；外部持有的旧immutable `Arc`仍可安全读取且释放后自然回收；retired `QueryNodeId`不能解析到新slot；incremental结果继续与clean recomputation等价。

进展（2026-07-22）：R-1 建立通用 query-key quiescent retirement，并接入 source revision owner。`QuerySession` 以 activity/retirement gate阻止回收与 query execution、validation、invalidation、trace并发；retirement请求会先阻止新query进入，再等待全部active query退出，且禁止在同session query内部发起回收。`QueryDb::retire` 先使root及dependents失效，再从typed cache、live slot identity table和dependency graph的forward/reverse edges物理删除节点；slot index改为单调分配且永不复用，旧`QueryNodeId`不会指向新slot，potentially-outdated parent遇到已退休dependency会按invalid input重算而非panic。loader `set_source` 在失效stable source root后精确退休旧revision的`ParsedModuleQuery`、`SyntaxModuleQuery`、`ModuleDeclarationsQuery`、`ProviderSummaryQuery`与`ModuleFacadeFactsQuery`。100轮body edit回归证明这五类query各只保留一个current live slot，同时初始`Arc<ParsedModule>`仍可安全读取；cache owner不再伪装历史读者，也没有加入fallback、双轨API或定期重建session。严格workspace/all-targets/all-features Clippy无warning；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，`nia-query` 54、loader-query 48、compiler-query 135、Driver 485与LLVM 177项通过。该补充阶段现约40%，整份roadmap加权约68%；下一切片收口`ModuleGraphRevisionQuery(ProviderFactRevision)`及provider event history，再为共享`NodeStore`建立显式revision owner边界。

进展（2026-07-22）：R-2 删除provider event history与`ModuleGraphRevisionQuery` predecessor链。`ProviderFactStore`不再持有`HashMap<ProviderFactRevision, Event>`，只保存current canonical demand set及至多一个尚未materialize的additive transition；graph query用上一immutable snapshot增量构造current graph以保持既有`ModuleId`，成功后将transition压成current full state。`QueryDb::seal_and_retire_predecessor`只允许无fingerprint、已拥有完整输出的query lineage使用，并在quiescence下断言predecessor只有current一个dependent，随后切断edge并从typed cache、slot table和dependency graph物理删除predecessor；没有untracked get、历史fallback或周期性session重建。source reset则直接退休旧graph revision。100轮provider add/reset回归证明始终只有一个live `module_graph_revision` slot、零retained transition且无revision→revision edge，同时外部初始graph snapshot保持可读；既有additive graph identity回归继续证明旧module handles稳定。`nia-query`55、loader-query49、compiler-query135与Driver485项通过，严格workspace/all-targets/all-features Clippy无warning。该补充阶段现约65%，整份roadmap加权约69%；下一切片为共享`NodeStore`建立revision-owned locator/payload回收边界。

进展（2026-07-22）：R-3 关闭revision retirement补充阶段。`NodeStore`不再以session级`HashMap + Vec`永久拥有全部`VersionedNodeKey`，而是为每个活跃`SourceVersion`建立独立revision shard；中央store只登记current shard与其live `NodeIndex`，`NodeMap`/`NodeOriginTable`只持有自身实际使用的shard，旧公开`NodeStoreAppend`写入口也已删除。`NodeId`继续使用不复用的monotonic index，无需generation；退休会从中央revision/index表物理删除全部locator，新建同version shard也获得不同index。外部旧query product通过自己的immutable shard继续解析NodeId，但current store lookup明确返回`None`，没有weak fallback、历史索引或session重建。新增`QueryDb::retirement_transaction`，让source mutation、provider reset、root invalidation、五类revision query退休和node shard退休处于同一个session-wide quiescence窗口，删除原逐项retire之间可重新计算同key的竞态。100轮同结构source edit回归证明active node revision恒为1、locator数恒定、query slot数恒定，初始parsed product仍可独立读取；全仓审计确认其余`VersionedNodeKey` map只属于单次builder或cache-owned current value，不是history owner。无环境变量、无线程参数的原样`cargo test --workspace`完整通过，`nia-node-id`15、`nia-query`56、loader-query49、compiler-query135、Driver485与LLVM177项通过，CLI commands 50项587.12秒完成，全部emit-exe/integration/doc tests通过。该补充阶段至此100%，整份roadmap加权约70%；下一切片进入Phase F，先量化checked/lowered/static/backend IR的owner与峰值生命周期，再确定typed handle及owned extraction切分。

### 阶段 F（P1）：IR ownership 与 item 粒度

1. 固化 `CheckedBody`、`LoweredFunction`、`StaticInit` 的正式职责与 ownership contract。
2. checked body、lowered function 改为per-item query-owned product；只有存在真实graph edge时才引入owner-scoped typed handle。
3. 对单消费者 IR 实现 owned extraction/steal 或 generation replacement。
4. backend lowering 改为 per-mono-item/per-CGU。
5. 让旧 IR 在消费后释放，测 peak live bytes。

Acceptance：BackendProgram 不再包含所有 function body 深树和 interner；peak RSS 显著下降；单 body 变更不重新 lower 无关 module bodies。

进展（2026-07-22）：F-1 删除backend lowering输入组装与instance discovery中的无语义`FunctionBody`副本。`LoweredFunctionBodiesQuery(ModuleId)`仍是本阶段待拆的module-owned query product，但`BackendLoweringIndexes::program_function_bodies`现只保存指向这些query-owned body的借用，不再在backend lowering开始前深拷贝全程序Function IR；公开`BackendLowerModuleInput`类型同步要求借用索引，测试fixture不能再传入第二份owned map。普通函数materialization直接从source body借用并只在写入`BackendFunction`时复制一次；generic template消费自身已复制的body，instance引用发现直接借用刚写入`BackendFunctionInstance`的body，不再为临时扫描额外clone。地址一致性回归证明program index与query product指向同一allocation。身份决策同步明确：`GlobalDefId`继续表示函数语义身份，后续`LoweredFunctionId`只表示owner-scoped storage位置；短期单调用消费者使用借用，不把所有“类似指针”的关系机械ID化。严格workspace/all-targets/all-features Clippy无warning；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，backend-lower96、compiler-query136、Driver485与LLVM177项通过，CLI commands 50项595.22秒完成，全部emit-exe/std runtime/integration/doc tests通过。Phase F现约25%，整份roadmap仍约70%；下一切片建立per-function lowered product/store与typed handle，随后再定义唯一消费时的generation/extraction协议。

进展（2026-07-22）：F-2 为lowered Function IR建立owner-scoped typed store并删除旧owned map契约。`nia-function-ir`新增immutable `FunctionBodyStore`与8-byte `FunctionBodyId { store,index }`；builder按`GlobalDefId`写入payload与handle map，foreign handle在其他store明确解析失败，独立store的语义比较按def/body而不是本地handle值。Store由`LoweredFunctionBodiesQuery`的immutable product直接拥有，不是session级append-only arena；query product释放时payload、def index和handle owner整体释放，不需要另设revision history/retirement表。`nia-function-lower::LoweredFunctionBodies`现唯一发布该store，compiler-query重复同名DTO和`HashMap<GlobalDefId, FunctionBody>`转拷贝已删除；backend module-local contains/get/validation全部读取store，跨模块program index仍只借用payload。严格workspace/all-targets/all-features Clippy无warning，function-ir7、function-lower49、backend-lower96与compiler-query136项定向测试通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，Driver485、LLVM177、CLI commands 50项（589.81秒）与process 39项（156.88秒）均通过，全部emit-exe/std runtime/integration/doc tests通过。`FunctionBodyId`只允许表达当前module store内的storage位置；per-function query落地后必须按真实consumer重新审计，若没有跨结构local-handle边则连同module aggregate物理删除，不能以潜在用途或兼容为由保留。Phase F现约30%，整份roadmap仍约70%；下一切片把module-keyed lowering拆成per-function query/product，使单body变化能够green复用无关lowered body，再迁移checked `TypedBody` owner。

进展（2026-07-22）：F-3 将checked/lowered function IR切到真实item级red-green边界。`BodyIr.function_bodies`的payload改为`Arc<TypedBody>`，module/executable aggregate与新增`ExecutableFunctionBodyQuery(GlobalDefId)`共享同一不可变body allocation，不做深拷贝；该semantic-value query即使因上游aggregate变化而重执行，也会为内容未变的body保留fingerprint。`LoweredFunctionBodyQuery(GlobalDefId)`只依赖对应checked-body item product并直接拥有单个`FunctionBody`；monomorphization和backend通过`get_many`共享相同cache handle，backend module-local与跨模块查找统一使用一次调用栈内的借用program index。两函数增量回归证明修改`helper`后lowering累计执行数只从2增至3，未改`main`保持同一cache allocation并green validation。按F-2预设的consumer审计确认没有结构保存local body handle，因此`FunctionBodyId`、`FunctionBodyStore`、module-keyed `LoweredFunctionBodiesQuery`、batch DTO/API及backend local store入口全部物理删除，不保留adapter。定向测试body-check139、function-lower49、backend-lower96、compiler-query137项通过；严格workspace/all-targets/all-features Clippy无warning，无环境变量、无线程参数的原样`cargo test --workspace`完整通过，Driver485、LLVM177、CLI commands 50项（630.04秒）与process 39项（159.27秒）均通过，全部emit-exe/std runtime/integration/doc tests通过。Phase F现约40%，整份roadmap约71%；下一切片将checked-body生产本身从module/executable aggregate拆到item owner，再审计`StaticInit`与BackendProgram的owned extraction/释放边界。

进展（2026-07-22）：F-4 将static initializer接入正式item ownership边界并删除backend的aggregate入口。`BodyIr.global_inits`改为共享`Arc<StaticInit>` payload，新增semantic-value `ExecutableStaticInitQuery(GlobalDefId)`；同一revision内aggregate、item query与backend borrowed index指向同一allocation，两static增量回归证明只改其中一个时未改值保持语义相等，item query按semantic-value契约复用fingerprint。backend input组装持有query handles并建立一次调用栈内的`GlobalDefId -> &StaticInit`索引，`BackendLowerModuleInput.body_ir`与`nia-backend-lower`对`nia-body-ir`的生产依赖已物理删除。普通global只在写入`BackendGlobal`时深拷贝一次；generic global因类型替换必须生成独立owned tree。static simplifier现消费owned tree并同时返回changed标记，删除了仅为前后比较而做的整树clone，uniform array压缩也直接移动首元素。consumer审计未发现独立storage identity，因此没有引入`StaticInitId`或store。定向测试body-check139、backend-lower96、compiler-query138项通过；严格workspace/all-targets/all-features Clippy无warning，无环境变量、无线程参数的原样`cargo test --workspace`完整通过，Driver485、LLVM177、CLI commands50项（615.83秒）与process39项（164.40秒）均通过，全部emit-exe/std runtime/integration/doc tests通过。Phase F现约48%，整份roadmap约72%；下一切片应把checked body/static init producer从executable aggregate拆到item owner，再将backend materialization拆为per-item/per-CGU产品以建立可测的及时释放边界。

进展（2026-07-22）：F-5 将checked `TypedBody`的生产从executable aggregate迁到正式item query。fixed point最终check改为`FactsOnly`，新增`ExecutableCheckedModuleFactsQuery`只发布无function body的checked module facts、runtime function key和可达body module集合；`ExecutableFunctionBodyQuery(GlobalDefId)`从目标函数的frozen `FunctionSemanticFacts`构造单函数prechecked输入，以`BodyOnly`产品只lower该函数，`ExecutableCheckedModulesQuery`反向从item products组装当前aggregate view，不存在从旧aggregate提取body的路径。没有side facts的空函数会以空function-facts输入正常产出body，不能把map缺项解释为body缺失；专门回归同时锁定item、aggregate和backend三层都保留该函数。type-only模块继续使用signature fact模式，不会因item producer误执行完整type/value resolution；reachability、worklist与extension lookup依赖现在明确落在facts query，aggregate只依赖facts和item bodies。两函数增量回归证明facts aggregate的function body恒为空、aggregate与item product共享同一allocation，单次body edit会使两个checked-body producer累计执行4次，但semantic equality只让变化函数的lowered query执行，lowering累计为3次且未改函数复用原cache allocation。这一结果明确了当前边界：生产已经item-owned，输入的semantic facts仍是aggregate；在facts进一步item化前不能宣称checked analysis完全item-incremental。函数根body已由`GlobalDefId`唯一定位，嵌套body没有独立cache/失效/释放或cross-structure边，因此不引入同义`TypedBodyId`；`Arc<TypedBody>`只服务当前item query与aggregate view的真实并存owner，后续删除aggregate时必须重新审计并删除无消费者的`Arc`。定向测试body-check139、function-lower49、backend-lower96、compiler-query139项及CLI object warning回归通过；严格workspace/all-targets/all-features Clippy无warning，无环境变量、无线程参数的原样`cargo test --workspace`完整通过，Driver485、LLVM177、CLI commands50项（849.11秒）与process39项（233.93秒）均通过，全部emit-exe/std runtime/integration/doc tests通过。Phase F现约56%，整份roadmap约73%；下一切片把`StaticInit`生产从aggregate facts迁到item owner，随后拆分per-item/per-CGU backend materialization和及时释放边界。

进展（2026-07-22）：F-6 将完整`StaticInit` payload的生产从executable facts fixed point迁到正式item owner。`FactsOnly`不再把initializer tree写入`BodyIr.global_inits`，只在`ExecutableFactModuleState`中保存新的`StaticInitRefs`轻量摘要；函数/global引用由`StaticInit::refs()`统一提取，zero-count repeat不产生虚假可达边，`nia-executable-facts`原重复递归实现已删除。摘要参与fresh/incremental reachability合并，owner function失效时同步移除其local static摘要；最终facts product发布排序后的`runtime_globals`且function/static payload均为空。`ExecutableStaticInitQuery(GlobalDefId)`以`StaticInitOnly`从frozen checked facts单次materialize目标global，跳过重复type-check；local static会把其owner function冻结的node facts临时提升到item lowering view，associated-function pointer回归证明该路径既能产出正确initializer也能驱动被引用函数进入backend。`ExecutableCheckedModulesQuery`只从function/static item products反向组装aggregate，同一revision的aggregate、item query与backend继续共享同一allocation，不存在从旧facts aggregate提取payload的路径，也没有引入同义`StaticInitId`或store。当前诚实边界是facts阶段为复用唯一的static-data representability诊断仍会临时lower一棵tree、提取摘要后立即释放；未来若要消除瞬时allocation，应为同一lowering实现增加payload/ref sink，不能复制第二套引用发现语义。定向测试static-ir2、body-check139、backend-lower96、compiler-query141项通过；严格workspace/all-targets/all-features Clippy、fmt与diff check通过，无环境变量、无线程参数的原样`cargo test --workspace`完整通过，Driver485、LLVM177、CLI commands50项（1044.90秒）与process39项（362.71秒）均通过，全部emit-exe/std runtime/integration/doc tests通过。Phase F现约64%，整份roadmap约74%；下一切片拆分per-item/per-CGU backend materialization并建立旧checked/lowered/static IR的可测及时释放边界。

进展（2026-07-22）：F-7 先为backend拆分建立真实production ownership边界。原本只在`cfg(test)`登记的`MonomorphizationQuery`与`BackendLoweringQuery`现进入正式compiler query registry，production `CodegenProgramQuery`不再在自身provider内直接执行这两个阶段，而是持有它们各自的cache-owned product；backend lowering同时正式记录对monomorphization的parent edge。回归锁定`codegen_program -> monomorphization`、`codegen_program -> backend_lowering`与`backend_lowering -> monomorphization`三条依赖，证明codegen字段与直接query结果共享同一`Arc` allocation，重复获取不会重执行三个阶段；既有per-function复用回归也改为要求lowered-body边由monomorphization/backend各自拥有，不再保留旧codegen直连形状。该切片没有伪称item化：backend query仍一次组装全程序module inputs，并在`nia-backend-lower`内部执行跨模块function/global-instance fixed point、module optimization、layout/vtable completion与DCE；下一步必须先提取deterministic global backend item plan，再让immutable module/CGU query消费闭包后的item集合，禁止缓存不完整module product后从外部worklist继续mutation。backend-lower96、compiler-query142项定向测试与严格workspace/all-targets/all-features Clippy、fmt、diff check通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1086.95秒）与process39项（390.59秒）均通过，全部native integration与doc tests通过。Phase F现约68%，整份roadmap约74%。

进展（2026-07-22）：F-8 将frontend已经闭包的backend source items拆成正式per-module semantic plan。新增`BackendModuleSourceItemPlanQuery(ModuleId)`，从`ExecutableCheckedModuleFactsQuery`投影并在owner内按module过滤、排序、去重function/global/struct/union key；executable backend module input直接消费这些只读slice，不再读取`CheckedModule`中的reachable set。`EntryPoints`首轮现在精确物化plan中的普通source function；跨模块扫描遇到已有query-owned body的source function不再回灌外层fixed point，因为其owner module已在首轮计划，extern、mono function/global instance、vtable与instance诱发闭包仍由现有全程序fixed point处理。`Bare/FunctionBodies`、standalone `Public`与type-only `NoFunctions`策略保持原语义。双模块回归证明entry plan不会吸收child aggregate root、四类key均严格module-local且有序，`main -> helper`与跨模块调用按plan物化而unused function被排除，并锁定`backend_lowering -> backend_module_source_item_plan -> executable_checked_module_facts`依赖链。严格workspace/all-targets/all-features Clippy、fmt与diff check通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，backend-lower96、compiler-query143、LLVM177、Driver485、CLI commands50项（1093.89秒）与process39项（401.10秒）均通过，全部native integration与doc tests通过。该切片只完成frontend source subplan，不能宣称完整global backend item plan或per-CGU materialization；下一切片应提取instance/vtable/global-instance闭包的deterministic plan，再定义immutable module/CGU产品。Phase F现约72%，整份roadmap约75%。

进展（2026-07-22）：F-9 将frontend monomorphization aggregate到backend module的function-instance输入收敛为正式semantic plan。新增`BackendModuleFunctionInstancePlanQuery(ModuleId)`，验证目标属于当前executable facts，按definition owner module过滤实例，以deterministic mono symbol排序，并在重复完整semantic instance key时明确ICE；跨模块回归锁定child generic definition进入child plan、entry plan为空而`arg_module_id`仍保留entry调用方上下文。`BackendLowerModuleInput`只接收窄`BackendFunctionInstancePlan` slice，`nia-backend-lower` production dependency与公开lower API中的完整`Monomorphization`均已删除，测试fixture仅通过dev-dependency把collector结果投影成真实输入。backend组装的七层`zip`同步改为显式平行输入长度验证与index materialization，任何产品缺失不会被静默截断；query依赖回归锁定`backend_lowering -> backend_module_function_instance_plan -> monomorphization`且不存在backend到monomorphization的直接边。backend-lower96与compiler-query143项定向测试、严格workspace/all-targets/all-features Clippy、fmt与diff check通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1061.00秒）与process39项（372.47秒）均通过，全部native integration与doc tests通过。该计划仍只覆盖frontend collector已发现的function instances；body/vtable在backend中诱发的动态function instances、generic global instances、vtables及其跨模块闭包仍在aggregate fixed point内，不能宣称完整backend item plan。Phase F现约75%，整份roadmap约76%；下一切片应先提取global-instance与vtable的deterministic plan/closure语义，再定义immutable module或CGU materialization产品。

进展（2026-07-22）：F-10 将backend跨模块动态闭包的三条散落队列收敛为单一iteration snapshot。`PendingForeignBackendItems`统一拥有source function、function instance与global instance的pending/queued状态，每轮先按完整semantic key去重并按definition owner生成`ForeignBackendItemPlan`，再按module plan顺序消费；本轮materialization新发现的边只能进入下一轮snapshot，不再修改正在遍历的batch。普通function refs由`HashSet`改为`BTreeSet`，owner batch继续显式排序，消除hash seed对closure顺序的影响；重复module owner和缺失foreign owner不再被`HashMap`覆盖或`continue`静默丢弃，而是明确ICE。两条新回归锁定跨owner分组/排序/去重和missing-owner拒绝，backend-lower98、compiler-query143项及严格workspace Clippy、fmt、diff check通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1093.28秒）与process39项（380.15秒）均通过，generic local static、跨模块trait-object adapter/dynamic dispatch及全部native integration/doc tests通过。审计同时确认不能从pre-backend `FunctionBody`伪造完整global-instance plan：generic local static在source IR仍是普通local-static identity，具体`GlobalInstance` key只在function template substitution后出现；vtable诱发instance同样依赖post-substitution body。该snapshot仍是aggregate调用栈内的convergence boundary，不是query product；下一切片必须把substitution/planning变成可独立缓存的闭包结果，不能增加一个永远为空或遗漏动态边的frontend plan。Phase F现约77%，整份roadmap仍约76%。

进展（2026-07-22）：F-11 将function/global instance的post-substitution payload与引用发现收敛为同一个closed materialization delta。`FunctionInstanceMaterialization`与`GlobalInstanceMaterialization`在concrete body/init生成时同步拥有新backend payload及其完整`FunctionRefs`；nested instance递归和外层module closure消费同一份discovery结果，不再先丢失引用事实、随后扫描全部既有function/global instance重建。跨模块追加source/function/global item统一从当前module已有semantic key构造deduplicated worklist，再只注入本次delta；旧`append_function_instances`与`lower_additional_reachable_functions_from_instances`整条aggregate-rescan路径物理删除。该修正也让foreign global-instance materialization产生的function/global边立即进入下一轮deterministic snapshot，而不是等待无关function batch触发偶然重扫。新增compiler-query回归锁定entry调用child generic function后，child owner能从substituted body继续生成i32/u64两份local-static global instance；backend-lower98、compiler-query144项与严格workspace Clippy、fmt、diff check通过。无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1036.63秒）与process39项（369.35秒）均通过，generic local static、跨模块trait-object adapter/dynamic dispatch及全部native integration/doc tests通过。delta只在单次backend调用栈内move，不增加同义ID、store或`Arc`；它已经是未来query可缓存的最小closed substitution result，但当前仍不是query product，vtable discovery也仍扫描function/function-instance aggregate。下一切片应让materialization delta显式携带vtable requests/entries并建立完整global backend item plan，再定义per-module/per-CGU query消费边界。Phase F现约80%，整份roadmap约77%。

进展（2026-07-22）：F-12 将vtable requests/entries并入统一`BackendItemDiscovery` closed delta。普通source function与每个concrete function instance都只扫描刚生成的body，一次产出完整`FunctionRefs`和trait-object vtable entries；nested instance扩张、module-local closure与跨模块下一轮snapshot消费同一结果。vtable entry诱发的source/default-method instance边在append delta时立即进入worklist，module中仅按vtable semantic key去重，不再扫描全部function/function-instance aggregate。旧aggregate collector入口、两处`collect_new_trait_object_vtables`调用和优化后的`complete_reachable_backend_items` safety rescan均已物理删除；devirtualization、cross-function constant propagation与inlining只允许删除或复制已发现边，不是新的semantic reachability producer。新增compiler-query回归让generic `call[T]`的substituted body执行trait-object coercion，锁定concrete function instance、vtable和default-method instance都唯一materialize；backend-lower98、compiler-query145项及严格workspace/all-targets/all-features Clippy、fmt、diff check通过。无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1065.13秒）与process39项（374.14秒）均通过，generic local static、跨模块trait-object adapter/dynamic dispatch及全部native integration/doc tests通过。delta仍只在单次backend调用栈内move，没有增加同义ID、store、`Arc`、fallback或兼容路径；下一切片应把这份完整closure plan提取为独立query product，再让immutable per-module/per-CGU materialization消费。Phase F现约85%，整份roadmap约78%。

进展（2026-07-23）：F-13 为backend closure建立唯一的post-closure module finalization边界。审计发现旧`lower_module`会在outer foreign fixed point之前执行devirtualization、cross-function constant propagation、inlining、DCE与aggregate/layout completion；之后跨模块动态加入的source/function/global instances不会再经过module pass，只会在每个delta后重复扩张struct instances并整份重建layouts。`lower_initial_module`现在只materialize module-local闭包，outer deterministic snapshots完全排空后才对每个owner调用一次`finish_module`，统一优化全部初始和late items、执行DCE、完成reachable aggregates/instances并生成最终layouts；additional-item路径中的重复instance/layout rebuild已删除，lowerer/module数量不一致会明确ICE。跨模块vtable回归加入module-local constant helper并在O1下锁定vtable诱发的default-method instance收敛后才触发leaf inline，证明late materialization真实进入finalization。backend-lower98、compiler-query145项与严格workspace/all-targets/all-features Clippy、fmt、diff check通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1026.85秒）与process39项（360.41秒）均通过，全部native integration与doc tests通过。该切片没有提前制造`BackendItemPlanQuery`：closure仍依赖调用栈内`ModuleLowerer` substitution/trait caches，若在旧顺序上缓存会固化半闭包且优化状态不一致的module；现在才具备提取纯closed plan与immutable per-module consumer的稳定前提。没有新增ID、`Arc`、fallback或兼容入口。Phase F现约87%，整份roadmap仍约78%。

进展（2026-07-23）：F-14 将完整backend closure从隐式调用栈状态提取为明确的consuming `BackendItemPlan`。`plan_backend_program_with_timings`现在只负责input validation、shared indexes、initial materialization和cross-module deterministic closure，产出包含全部未finalize module items、diagnostics和materialization-time optimization changes的owned plan；plan不实现`Clone`，也不保留`ModuleLowerer`或借用substitution/trait cache。`finalize_backend_item_plan_with_timings`验证module owner数量与顺序，从原输入重建只读finalizer/aggregate-source indexes，然后move plan modules执行唯一post-closure finalization；compiler production provider已显式分两个timing stage调用，一站式lowering入口仅作同一正式编排。backend-lower98、compiler-query145项与严格workspace/all-targets/all-features Clippy、fmt、diff check通过；无环境变量、无线程参数的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1030.52秒）与process39项（362.33秒）均通过，全部native integration与doc tests通过。本切片刻意尚未建`BackendItemPlanQuery`：现有query `get`只发布共享`Arc`，若直接缓存plan就只能在finalization前深clone整份Backend IR，这会违背本阶段ownership目标。下一切片必须先建立quiescent且dependency-safe的query-owned extraction，或进一步拆成可廉价共享的module/CGU product；禁止增加clone fallback、双轨API、同义ID/store或无consumer `Arc`。Phase F现约89%，整份roadmap仍约78%。

进展（2026-07-23）：F-15 为query engine加入显式storage policy并把closed backend plan接入正式query graph。默认`CacheOwnedArc`保持immutable shared cache；新的`SingleConsumerOwned`允许非`Clone` value由provider直接move给唯一consumer，slot消费后只保留无payload的`Consumed`状态、执行统计和依赖边。再次直接请求会重新生产，上游失效仍沿保留的graph边传播到cached consumer；owned query禁止fingerprint，普通`get`与`get_owned`会明确拒绝错误policy，没有隐式clone或shared fallback。新增`BackendItemPlanQuery`作为该policy的首个production用户，`BackendLoweringQuery`经`get_owned`消费plan并在作用域受限的输入重建中完成finalization；依赖链明确为`backend_lowering -> backend_item_plan -> backend_module_source_item_plan/backend_module_function_instance_plan`，重复backend/codegen请求复用finalized product，plan仅执行一次。架构文档同步删除“尚非query product”的过渡描述，并记录owned storage contract。backend-lower98、query60、compiler-query145项及严格workspace/all-targets/all-features Clippy、fmt、diff check通过；无环境变量、无线程参数、无timeout的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1030.70秒）与process39项（365.56秒）均通过，全部native integration与doc tests通过。没有新增ID、store、`Arc`、clone fallback、双轨API或兼容入口。Phase F现约93%，整份roadmap约79%；下一切片应以closed plan为边界拆出immutable per-module/per-CGU materialization product及及时释放验证，而不是重新引入aggregate ownership。

进展（2026-07-23）：F-16 将closed aggregate plan内部物理拆成不可`Clone`的`BackendModuleItemPlan`。全程序closure仍在唯一truth source中收敛，只有完全闭包后才把每个owner module封装为独占plan unit；finalization先验证owner顺序并重建只读program indexes，再通过`into_iter`逐个move module plan执行optimization、DCE、aggregate/layout completion，不存在shared module plan或第二份store。`BackendLowering`、`BackendProgram`与`BackendModule`的顶层深`Clone`同时删除，workspace all-target check证明没有真实consumer依赖该能力；allocation-identity回归锁定functions/globals两个大型Vec从plan进入finalized module后backing address不变，直接证明该边界没有深拷贝。backend-lower98、compiler-query145、LLVM177项定向套件与严格workspace/all-targets/all-features Clippy、fmt、diff check通过；无环境变量、无线程参数、无timeout的原样`cargo test --workspace`完整通过，Driver485、LLVM177、CLI commands50项（1051.94秒）与process39项均通过，全部native integration与doc tests通过。没有新增ID、`Arc`、clone fallback、兼容adapter或并行aggregate。Phase F现约95%，整份roadmap仍约79%；下一切片把现有module consuming unit提升为独立可调度的module/CGU query product，并加入及时释放与peak-live-bytes观测，不能把物理分区误称为已经完成CGU work product。

进展（2026-07-23）：F-17a 为closed module unit建立正式的query-owned fan-out。`nia-query`新增`ExternallyPublished` provider contract：tracked producer可把非`Clone`值直接写入`SingleConsumerOwned` slot，slot以原始`V`而非`Arc`暂时持有payload，`get_owned`按值move后进入无payload的`Consumed`状态；producer失效会丢弃尚未消费的payload并沿正式edge失效下游，未发布或已消费的key拒绝直接读取。`BackendItemPlanQuery`的唯一consumer现在只拆aggregate一次，再按`ModuleId`发布`BackendModuleItemPlanQuery`，所有module slot随后进入finalization；module query不执行全程序closure，不存在side store、同义ID、共享payload或clone fallback。旧aggregate finalization API已删除，唯一finalizer明确接收finalization metadata与module plans。query drop回归锁定producer失效立即释放未消费payload，compiler回归锁定module slot消费后为空、只失效backend时可重新发布、失效aggregate时module/backend完整传播；registry也明确区分key-executed与externally-published provider。query63、backend-lower98、compiler-query146项定向套件与严格workspace/all-targets/all-features Clippy、fmt、diff check通过；无环境变量、无线程参数、无timeout的原样`cargo test --workspace`完整通过，LLVM177、Driver485、CLI commands50项（1067.17秒）与process39项（379.20秒）均通过，全部native integration与doc tests通过。本切片建立的是module-keyed owned product，不宣称finalizer已并行、source module已成为合理CGU或已有peak-live-bytes数据。Phase F现约97%，整份roadmap仍约79%；下一切片把每个module finalization变成可调度任务，重构program-wide只读finalizer context以安全共享，并用allocator/timing counter记录fan-out前后真实peak live bytes，再进入deterministic CGU partition。

进展（2026-07-23）：F-17b 建立真实allocator live/peak基线并接入module fan-out观测。`CountingAllocator`现在从进程启动起以原子current-live计数跟踪成功allocation/deallocation/reallocation，detail timing以启动时已存活heap为基线记录区间peak；report新增`allocator.live_bytes`与`allocator.peak_live_bytes`，没有`perf-alloc` instrumentation时仍不输出伪造零值。backend在aggregate plan取出后、全部module slot发布后、全部slot按值消费后三次记录current/peak，instrumented minimal backend emit已验证六个counter存在且`query.value_clone_bytes=0`；baseline runner现在要求peak-live counter，comparator以既有allocation阈值同时守卫累计allocated与peak-live。该指标只覆盖Rust global allocator，LLVM/native allocation继续由max RSS约束。timing12项、Python perf工具6项、perf-alloc CLI timing回归、compiler module-plan失效回归与严格workspace/all-targets/all-features check/Clippy、fmt、diff check通过；无环境变量、无线程参数、无timeout的原样`cargo test --workspace`完整通过，compiler-query146、LLVM177、Driver485、CLI commands50项（1072.32秒）与process39项（378.10秒）均通过，全部native integration与doc tests通过。Phase F现约98%，整份roadmap约80%；下一切片只推进module finalizer的可调度任务边界与program-wide readonly context，不把source module直接冒充最终CGU。

进展（2026-07-23）：F-17c 将post-closure module finalization拆成明确的task-shaped ownership边界。新的`BackendProgramFinalizationContext`只拥有program-wide readonly indexes、canonical type store借用、optimization policy与timing flag；`BackendLowerModuleInput`的symbol/defs动态lookup显式要求`Sync`，编译期回归同时锁定context和input为`Send + Sync`、独占`BackendModuleFinalization`为`Send`。每次`finalize_module`只消费一个不可clone的`BackendModuleItemPlan`，返回独占finalized module、diagnostics与optimization changes，不再由多个`ModuleLowerer`直接写共享聚合结果。唯一`finish_backend_module_finalizations`按task原始batch position原地恢复program顺序，并验证position连续、module owner匹配后才合并三类输出；反序完成回归证明module、diagnostics和pass-change顺序均不受完成时序影响，合并不建立第二份module owner map或side store。当前production仍串行调用该边界：module input借用callback-local query handles，而Phase E persistent executor接收`'static` closure；因此本切片不宣称已经并行，也不以scoped OS threads或无真实多consumer的`Arc`规避lifetime。定向测试、workspace/all-targets/all-features check与严格Clippy通过；Phase F仍约98%，整份roadmap仍约80%。下一切片应建立真实owned readonly task environment并接入统一executor，再以现有fan-out live/peak counter验证并发内存，之后进入deterministic CGU partition。

进展（2026-07-23）：F-17d 为single-consumer task product补齐统一persistent-executor batch入口。`QueryBatch`从固定`Arc<V>`输出泛化为直接承载任意`Send` output；公开`get_many`继续只为`CacheOwnedArc` key返回共享handle，新的`get_many_owned`只通过既有`get_owned` storage contract按值返回`Vec<V>`，二者复用同一queue、worker pool、jobserver permit、nested execution、panic汇聚、parent query stack与dependency/fingerprint merge实现，没有第二套executor、clone fallback或owned-result `Arc`。non-Clone顺序回归锁定结果按key顺序move；parent invalidation回归锁定owned batch的全部依赖边；parallelism回归锁定四个owned task在session budget 2下peak active严格为2。该原语解除module finalization结果无法经统一executor按值返回的runtime阻点，但尚未伪造compiler-side owned environment或finalization query；callback-local query handles的`'static` ownership仍是下一切片唯一临界问题。Phase F仍约98%，整份roadmap仍约80%；下一步应让真实多module consumers共享唯一owned readonly input/context，并以`get_many_owned`调度正式module-finalization query。

进展（2026-07-23）：F-17e 将source-module finalization接入正式production query调度。`nia-backend-lower`新增object-safe且`Sync`的唯一`BackendProgramFacts`契约，`BackendLowerModuleInput`中十余个`program_*` map/callback与trait impl字段已物理删除；compiler的`BackendLoweringInputsQuery`直接拥有既有query `Arc` handles、derived signature/index和稳定item key slice，不深拷贝`FunctionBody`或`StaticInit`，并作为全部backend program lookup的单一truth source。`BackendFinalizationTaskContextQuery`只让真实并发module consumers共享该owner与canonical `TypeStore` handle；每个`BackendModuleFinalizationQuery { module_id, position }`按值消费一个externally-published module plan，以`SingleConsumerOwned`返回不可clone结果。`BackendLoweringQuery`通过统一`get_many_owned`调度全部module keys，再按position确定性合并module、diagnostic与optimization changes；诊断路径不创建task context。双模块回归锁定每轮每module恰执行一次finalization query、最终module顺序稳定、module plan slot消费后无payload，并锁定`backend_lowering -> backend_module_finalization -> backend_module_item_plan/backend_finalization_task_context -> backend_lowering_inputs`失效链；backend-lower100与compiler-query146项通过。没有兼容字段、side store、clone fallback、同义ID或为单consumer增加的`Arc`。这是source-module task并行，不是最终CGU partition；Phase F约99%，整份roadmap仍约80%，下一切片应先用现有allocator三点counter量化并发live/peak，再开始deterministic CGU partition。

进展（2026-07-23）：F-17f 完成source-module并发finalization的独立peak-live验证。原三点snapshot的peak字段从整次detail tracking开始累计，会被更早阶段的高水位遮蔽，不能证明finalization窗口自身峰值；`nia-timing`现增加单一process-wide allocation live window，调用线程包围`get_many_owned`期间，所有query worker allocation通过同一atomic peak计数，输出start/end/peak/peak-growth，重叠窗口明确不合并且runner会拒绝缺失counter。perf suite新增第七个`module_backend` workload，复用维护中的多模块/std示例并只emit backend IR，避免LLVM allocation混入；三次instrumented release样本的start-live中位172,843,401 bytes、end-live中位173,099,883 bytes、peak-growth中位1,102,630 bytes（约0.64%），全程allocator peak中位187,088,115 bytes高于finalization窗口峰值，证明该workload的并发finalization没有制造新的Rust heap高水位。窗口结束净增中位约255 KB，来自cache-owned readonly context/query metadata与finalized result持有，不是body/static深拷贝；`query.value_clone_bytes`仍为0。timing13项、Python perf工具8项和compiler失效定向回归通过。Phase F仍按99%计，因为source module不是最终CGU；下一切片正式进入deterministic CGU partition设计与首个semantic partition product。

### 阶段 G（P1）：CGU、异步 codegen 与 work products

1. deterministic mono collection 与 CGU partition。
2. codegen task queue；frontend 与 LLVM overlap。
3. CGU fingerprint、object cache、incremental link inputs。
4. 记录 CGU reuse 与 invalidation reason。

Acceptance：多核 workload CPU 利用率明显提升；小改动只重建受影响 CGU；并行不会显著抬高 RSS。

进展（2026-07-23）：G-1 建立首个正式deterministic codegen partition product。`BackendLowering`在finalized module集合形成后唯一发布`CodegenPartitionPlan`；source unit使用`CodegenUnitId::SourceModule { module_id, ordinal }`，首版policy令每个拥有function/global/concrete instance/vtable定义的backend module对应ordinal 0，按typed identity排序而不是继承`BackendProgram.modules`输入顺序。plan entry只保存module index，不复制Backend IR；declaration-only module继续进入whole-program `ProgramIndex`提供跨unit声明/layout lookup，但不再伪装成IR/object work unit。LLVM IR与object入口现在必须显式接收并验证正式plan，旧直接遍历全部module及LLVM本地`module_has_object_definitions` truth source已物理删除；两类输出都携带typed unit identity。compiler builtins明确使用独立`CompilerBuiltins` synthetic identity，只在native lowering确有需求时加入。乱序/空定义过滤、plan-program mismatch拒绝、跨模块输出顺序及builtins role回归已锁定；审计还纠正了旧object过滤把带body的`extern fn`误判为纯声明的问题，ABI/linkage标志不再覆盖body存在这一真实definition事实。Backend IR 2项、backend-lower 100项、compiler-query 146项与LLVM 177项通过，workspace all-target/all-feature check、严格Clippy、fmt与diff check通过。当前`ModuleId`仍是进程内identity、source ordinal尚未细分且LLVM仍串行，全程序readonly index也仍先构建；因此本切片不宣称已有持久CGU key/fingerprint、细粒度partition、codegen task queue、work product cache或incremental link input。Phase F仍按99%计，Phase G约15%，整份roadmap约81%；下一切片应把source partition提升为独立可调度LLVM task输入，并拆分每unit emission result，同时保持whole-program declaration/layout facts只读共享，随后再建立CGU fingerprint与object work product。

进展（2026-07-23）：G-2a 将LLVM module-local工作拆成独立per-unit emission result边界。IR与native object两条路径都在whole-program validation之后，把一个正式`CodegenPartition`、readonly `ProgramIndex`和options交给单unit函数；该函数独占创建并消费LLVM `Context`/`ModuleCodegen`，只返回一个携带typed unit identity的output或一个diagnostic。外层不再内联持有unit-local LLVM状态，只按partition plan顺序聚合outcome，因此未来task完成顺序可以变化而公开输出顺序无需变化。LLVM 177项回归通过。该边界当前仍由串行loop调用，不是task queue；直接并行仍被借用型`ProgramIndex<'a>`阻挡，不能用per-task重建全程序index、深拷贝Backend IR或无真实consumer语义的`Arc`绕过。Phase G约20%，整份roadmap仍约81%；下一切片先把`ProgramIndex`的borrowed-value maps改为owner+position lookup，使唯一readonly task context可安全拥有既有`Arc<BackendLowering>`/`Arc<TypeStore>`，再把这些unit outcome提交到统一session executor。

进展（2026-07-23）：G-2b 删除`ProgramIndex`内部的self-referential borrowed-value maps。module、struct/union/enum、function/global、concrete instance、vtable及五类layout index现在只保存`module/item/layout position`，所有consumer通过accessor回到canonical `BackendProgram` allocation；exact key、equivalent type fallback与by-def iteration仍复用原语义，没有复制function body/static init/layout，也没有新增ID、store或`Arc`。validator、declaration、type/static-init/function codegen中直接读取index map的路径已全部迁移，map结构不再成为外部truth source。回归以指针相等锁定module/function-instance lookup返回原allocation，并以编译期guard锁定`ProgramIndex<'static>: Send + Sync`；LLVM 178项通过。此时index仍借用唯一program/type-store roots，所以尚不能进入要求`'static` closure的persistent executor；下一切片应让一个owned readonly task context持有既有`Arc<BackendLowering>`和`Arc<TypeStore>`并拥有position index，再开放统一executor的非query task batch，禁止重新引入per-task全程序index。Phase G约25%，整份roadmap约82%。

进展（2026-07-23）：G-2c1 从`QuerySession`提取统一non-query `run_tasks` batch。任意`Send + 'static` closure及其non-Clone owned output现在可直接复用既有persistent queue、worker pool、jobserver/process CPU budget、nested progress、panic汇聚和submission-order归并，不注册伪query node或dependency edge。`get_many`与`get_many_owned`也已改为调用同一个内部batch核心，只在query wrapper中安装/归并parent dependency stack，不再各自内联executor提交协议。顺序与budget回归已加入，nia-query 68项通过。该原语只解除LLVM task提交阻点，尚未宣称LLVM已并行；下一切片必须建立唯一owned readonly codegen context并把per-partition outcome真实提交到此入口。

进展（2026-07-23）：G-2c2 将IR与native object的正式partition outcome提交到compiler同一个`QuerySession::run_tasks`。无生命周期的`ProgramIndex`现在直接持有既有`Arc<BackendLowering>`与`Arc<TypeStore>`，只构建一次position maps后以单一`Arc<ProgramIndex>`供全部task只读共享；旧borrowed program/plan/type-store LLVM入口已删除，Driver与CLI必须保留产生`CodegenProgram`的同一session，不存在新建executor fallback。每个source task独占LLVM `Context`/`ModuleCodegen`，native task还独占`TargetMachine`；可选compiler-builtins也作为typed synthetic task进入同一ordered batch。memory permit从整个public call外层下沉到每个unit LLVM allocation边界，因此executor CPU budget与进程memory backpressure同时约束fan-out。task只返回typed owned output或diagnostic，completion乱序由submission slot隐藏；没有per-task index重建、Backend IR深拷贝、scoped thread或unsafe lifetime erase。LLVM 178项与workspace all-target/all-feature check通过，Phase F ownership边界至此100%，Phase G约35%，整份roadmap约83%。下一切片应定义stable CGU fingerprint及其输入闭包，再建立object work product owner/cache与incremental link input。

进展（2026-07-23）：G-3a 将CGU的session-local定位身份与持久身份彻底分开。`CodegenUnitId::SourceModule { ModuleId, ordinal }`只用于本次编译内定位；新的`CodegenUnitKey::SourceModule { SourceIdentity, ordinal }`使用规范化source identity，可跨module handle重新分配稳定复现。`BackendModule`显式携带source identity，partition plan按stable key排序并拒绝重复key，不再让进程内`ModuleId`决定公开输出顺序；IR/object output同时携带runtime ID与stable key，compiler builtins使用独立synthetic key。反序module ID回归证明stable order不受allocator影响，重复source key回归锁定唯一性；旧依赖`outputs[0]`代表entry module的测试已改为按stable source key选择，不保留owner-order假设。Backend IR 4项、backend-lower 100项、compiler-query 146项通过。该切片只建立未来work-product identity，不把identity冒充content fingerprint；Phase G约40%，整份roadmap仍约83%–84%。下一步必须定义versioned CGU fingerprint domain，覆盖unit-owned Backend IR、referenced declaration/layout/ABI facts、optimization/target/compiler版本输入，并用差分回归证明输入变化边界，然后才能建立object cache。

进展（2026-07-23）：G-3b 建立正式versioned CGU content fingerprint输入契约。新增`CodegenUnitFingerprint`与独立source-unit/compiler-builtins domain；canonical encoder不使用`Debug`文本、标准库随机hash、`ModuleId`或`InternedTyId` slot，而把module引用映射为规范化`SourceIdentity`、def/const引用映射为stable source owner加local id、type handle递归展开为canonical `TyKind`并同时纳入resolved layout。每个source fingerprint覆盖本unit完整definition（function body/static init）、全程序declaration/ABI/vtable/const/layout surface、完整optimization policy、artifact kind、compiler package/schema及LLVM wrapper codegen ABI；native object额外覆盖实际构造`TargetMachine`的triple/CPU/features，compiler builtins则按独立domain覆盖精确symbol set。其他unit的body/init明确排除，避免无关实现改动全局失效；span与display-only local names明确排除。IR/object output都携带该content identity，计算发生在LLVM `Context`/module创建之前，不通过hash最终IR/object bytes伪装输入键。五项差分回归锁定ModuleId/type-store slot重分配和module输入乱序不变，initializer、跨模块ABI、optimization与native target变化必变，span变化不变；LLVM 183项、workspace严格all-target/all-feature Clippy、fmt与diff check通过。没有建立cache、fallback、第二份Backend IR owner或同义ID/store。Phase G约50%，整份roadmap约85%；下一切片只建立object work-product owner、schema namespace、原子publish/lookup与corruption miss语义，再接入incremental link inputs。

进展（2026-07-23）：G-3c 建立首个持久native object work-product owner。`nia-codegen-llvm`只定义单一`ObjectWorkProductCache`边界并在每个task内先计算target-specific fingerprint；命中发生在LLVM memory permit、`TargetMachine`、`Context`和module创建之前，直接恢复带current runtime ID/stable key/fingerprint的普通typed output，miss才进入唯一codegen路径并在成功后publish。filesystem策略唯一属于Driver：CLI `emit --obj/--exe --cache-dir`与build runner复用`.nia-cache/artifacts/objects/v1`；entry以完整128-bit fingerprint命名，binary envelope包含magic/schema、canonical unit key、payload length与domain-separated checksum。读取严格拒绝错误key/fingerprint、截断、尾随或checksum损坏，corrupt entry立即物理删除并作为miss，不读旧schema；publish在同目录写入并sync唯一temp后atomic rename，并发相同publisher收敛到immutable entry。cache I/O错误只损失reuse，不建立Backend IR/object fallback truth。timing新增`llvm.object_reuse_misses`并保留hits；mock命中回归证明不执行emission/publish，持久回归证明round-trip与corruption retirement/republish。真实跨Driver CLI首轮为hits 0/misses 1且取得memory permit，第二轮hits 1/misses 0且无memory permit/target/context/emit_object记录，输出object逐字节相同；body常量修改后重新miss并产生新work product。LLVM 184项、Driver 487项、build 15项及严格workspace Clippy/fmt/diff check通过。没有兼容manifest、旧schema reader、cache-side IR owner或按source handle索引。Phase G约62%，整份roadmap约87%；下一切片建立typed incremental link inputs，使link只消费ordered `(CodegenUnitKey, fingerprint, object)` product集合，再推进frontend/LLVM overlap与reuse invalidation reason。

进展（2026-07-23）：G-3c1 修复Bare native object路径违反source-item plan的aggregate泄漏。`BackendFunctionRoots::FunctionBodies`过去虽接收正式`reachable_structs/reachable_unions`，initial lowering却只在`EntryPoints/NoFunctions`模式使用它们，因而把已激活body module内所有非泛型struct/union发进Backend IR；`examples/00_minimal.nia`由此错误带入未使用的`fs::PathBuf`，再引用未物化的`string::StringView/StringBuf` owner并在LLVM validation失败。现在只要调用方提供typed aggregate plan，所有root policy都必须按该plan过滤initial source aggregate；跨模块closure、aggregate completion和LLVM validator保持严格，不新增缺失owner fallback。完整lowering fixture锁定`FunctionBodies`会保留计划内`Point`并删除计划外struct/union，真实`emit --obj examples/00_minimal.nia`成功且只剩既有unused-import warning。Phase G比例不因bug fix上调；下一切片仍是typed incremental link inputs。

进展（2026-07-23）：G-3d 建立正式typed incremental link input product。`nia-backend-ir`唯一拥有泛型`IncrementalLinkInputs<T>`，构造时强制stable `CodegenUnitKey`严格递增且唯一；每项把key、`CodegenUnitFingerprint`与对象载荷绑定，空集合也通过同一typed边界表达。LLVM native emission不再返回`LlvmObjectModuleOutput`/module vec，而直接发布`IncrementalLinkInputs<NativeObject>`；cache hit与miss进入完全相同的typed outcome。Driver写永久对象或链接临时对象时只把payload从bytes映射为`PathBuf`，逐项保留key/fingerprint；`ObjectArtifact.modules`、`WrittenObjectArtifact.paths`和Linker的裸`&[PathBuf]`入口已物理删除。Linker按typed集合既有顺序生成参数，不从文件名恢复身份、不自行排序、没有兼容adapter或第二份truth source。Backend IR 8项、LLVM 184项、Driver 488项、Linker 15项、build 15项与CLI commands 50项通过，workspace all-target/all-feature严格Clippy、fmt和diff check通过；ordered/duplicate/descending集合、linker顺序及Driver identity-preserving写盘均有回归，真实`emit --exe examples/00_minimal.nia`生成静态ELF并成功输出`hello from nia`。该切片只建立未来link fingerprint的输入闭包，不宣称已有partial relink或link-result cache。Phase G约68%，整份roadmap约88%；下一切片应基于完整typed input集合与link options/target/runtime定义versioned link-result fingerprint及复用owner，或推进frontend/LLVM overlap，不能退回裸path收集。

进展（2026-07-23）：G-3e1 建立versioned canonical `LinkResultFingerprint` contract。唯一encoder属于`nia-linker`，覆盖ordered CGU stable key/fingerprint、target、resolved linker path与binary contents、flavor、entry、mode、dynamic linker、默认/显式library path和rpath；使用固定tag与length-delimited值，不读取object payload/path、不哈希`Debug`文本或临时输出路径。sysroot、显式native library和raw linker arg可能引用尚未纳入闭包的外部文件，因此明确返回uncacheable而非产生不完整fingerprint。representation-independent、CGU/options/linker binary差分及opaque external input回归通过，Linker 18项通过。该切片只关闭link-result identity，不宣称已有cache owner或跳过linker；Phase G约72%，整份roadmap约88%–89%。下一切片必须由Driver建立唯一持久link-result owner并物理删除旧source graph/request manifest executable cache，不能让两套fingerprint并存。

进展（2026-07-23）：G-3e2 建立Driver唯一持久link-result owner并删除旧source cache双轨。`DriverConfig.object_cache_dir`直接替换为覆盖两类work product的`artifact_cache_dir`，没有旧字段alias；Driver在写入临时object与调用linker前计算正式fingerprint并尝试恢复，miss才执行完整link，成功后发布到`artifacts/links/v1`。binary envelope固定记录magic/schema、完整fingerprint、payload length与domain-separated checksum；同目录唯一staging加atomic rename保证未完成结果不可见，损坏entry会被物理删除后按miss重建，恢复文件显式赋予可执行权限，所有cache I/O错误只损失reuse。CLI与build runner原先基于source graph/request manifest的executable cache请求、snapshot、public API、manifest与测试已整体删除，二者现在复用同一个configured Driver，不读旧schema也没有fallback fingerprint。跨进程真实linker回归证明相同typed inputs只调用一次linker、`links/v1`只产生一个entry、恢复结果逐字节一致且可执行；CGU fingerprint encoder同时对全部hash-origin Backend IR集合按stable def/symbol/canonical key规范排序，修复了独立进程间owner/declaration编码漂移。该能力是完整link-result reuse，不是partial relinking；Phase G约78%，整份roadmap约89%。下一切片优先记录object/link reuse与invalidation reason，再细分source ordinal或推进frontend/LLVM overlap。

进展（2026-07-23）：G-4a 将object/link work-product复用观测从二值计数提升为typed lookup fact。`ObjectWorkProductCache::load`不再用`Option<Vec<u8>>`吞并entry状态，而是唯一返回`Hit/NotFound/Corrupt`；LLVM task边界再显式区分cache `Disabled`与`ReadError`，按全部unit聚合hits、misses及四类miss counter。Driver link-result restore同样返回typed outcome，并在正式timing传播下区分`Disabled/Uncacheable/NotFound/Corrupt/ReadError`，同时记录publish error；旧无timing的link-from-objects签名已直接迁移，没有wrapper。跨进程CLI回归锁定首轮object/link均报告not-found、次轮两层均报告hit，且真实linker仍只执行一次；corrupt entry测试锁定物理删除后的typed corrupt reason。reason只描述本次lookup可证事实，不写“latest fingerprint”manifest、不猜测哪个输入改变，因此观测数据不会成为第二cache truth source。LLVM 184项、Driver 486项与跨进程CLI定向回归通过。Phase G约82%，整份roadmap约90%；G-4仍需为CGU/link fingerprint设计versioned component decomposition，才能精确报告definition/ABI/options/target/linker等输入差分，之后再推进source ordinal或frontend/LLVM overlap。

进展（2026-07-23）：G-4b 为CGU fingerprint建立正式versioned component decomposition。source unit与compiler-builtins都由`policy/definition/declarations/target`四个独立domain组成，最终`CodegenUnitFingerprint`只通过固定v2 aggregate domain组合四个128-bit component；差分回归锁定body/init只改变definition、跨模块ABI/layout只改变declarations、optimization/compiler contract只进入policy、native triple/CPU/features只进入target，同时保持ModuleId/type slot/input order稳定。持久object cache升级为唯一`objects/v2/<stable-key-digest>/<full-fingerprint>.o`布局，v2 envelope记录并重新验证aggregate、四components、canonical key、payload checksum与content-addressed path，不读取v1。exact aggregate仍是唯一hit；miss只扫描同一stable-key目录内已验证的immutable entries，选择component差异最少且以aggregate确定性打破平局的版本报告原因，不写latest manifest、不覆盖旧版本、不让component metadata参与正确性判断。跨进程真实CLI三轮回归证明cold为not-found、第二轮完整hit、body edit只产生1个definition invalidation且policy/declarations/target均为0；旧object与新object并存，linker只因新完整fingerprint再运行一次。LLVM 184项、Driver 487项、严格workspace Clippy与CLI定向回归通过。Phase G约85%，整份roadmap约91%；下一切片应把link-result fingerprint拆为ordered inputs、target、linker binary与structured options components，并以同样的无index content-addressed布局报告link invalidation。

进展（2026-07-23）：G-4c 为link-result fingerprint建立对应的versioned component decomposition。`nia-linker`唯一生成`inputs/target/linker/options`四个独立component，并通过固定v2 aggregate domain形成唯一完整命中身份；ordered CGU stable key集合另生成只用于定位可比较历史版本的stable cache key，不包含mutable fingerprint或link option。Driver cache升级为唯一`links/v2/<stable-input-key>/<full-fingerprint>.link`布局，v2 envelope记录并重新验证cache key、aggregate、四components、payload checksum与content-addressed path，不读取v1。exact aggregate仍是唯一hit；miss只在同key目录选择component差异最少且以aggregate确定性打破平局的已验证immutable entry报告原因，没有latest manifest、覆盖写或第二truth source。跨进程真实CLI三轮回归证明cold为not-found、第二轮object/link完整复用、body edit只产生1个object definition与1个link inputs invalidation，其余object/link component均为0；新旧完整版本并存且真实linker只为新aggregate再执行一次。Linker 18项、Driver 488项、跨进程CLI定向回归通过。Phase G约88%，整份roadmap约92%；G-4 component reporting至此关闭，下一切片应在source ordinal细分与frontend/LLVM overlap之间按可测收益选择，不能把whole-result cache表述为partial relinking。

进展（2026-07-23）：G-5a 将source partition从隐式“整个BackendModule都是definition”改为显式position-only definition plan。每个`CodegenPartition`现在唯一列出本unit拥有的global initializer、global instance、function body、function instance与vtable位置；LLVM的三类definition动作及source definition fingerprint只消费该集合，全程序module仍是canonical Backend IR owner和declaration/layout只读来源，没有复制item、第二store或按handle恢复membership。原LLVM本地按`source.id`遍历并定义整个module的truth source已删除，plan/program validation会重建并比较definition membership。definition component切换到独立v3 domain，旧object不会被误命中。首个policy仍把每个有definition的source module完整放入ordinal 0，因此本切片只关闭后续细分的正确性边界，不宣称已有更细CGU或性能收益，Phase G与整体完成度暂不提高。Backend IR 8项与LLVM 184项通过；下一切片只应启用有上限且稳定的source bucket policy，并以同一module多unit、无重复definition、链接成功和单body edit局部object invalidation验收。

进展（2026-07-23）：G-5b 启用首个真实但有界的source ordinal policy。少于8个definition的module仍保持ordinal 0，达到阈值后最多使用4个固定bucket；普通global/function按source-local `DefId`取bucket，concrete global/function instance按既有stable mangled symbol取bucket，vtable固定进入bucket 0，禁止以`InternedTyId` slot分桶。只发布非空bucket；进入拆分状态后，增删item不会让所有后续ordinal整体位移。每个task仍只借用canonical module与唯一whole-program index，未复制Backend IR或声明面。LLVM回归直接锁定同一source产生多个非空unit且全部`define` symbol全局唯一；真实跨进程executable三轮回归在完整链接成功的同时证明第二轮全复用、同shape常量body edit只产生1个definition invalidation、declarations/policy/target均为0且其他bucket继续hit，link inputs只失效一次。Backend IR 9项、LLVM 185项、Driver 488项与CLI定向回归通过。Phase G约92%，整份roadmap约93%；下一切片应先量化拆分前后CPU并行度、LLVM memory wait与RSS，再据此决定推进frontend/LLVM overlap还是调整bucket阈值/上限，不能在无测量时宣称acceptance关闭。

进展（2026-07-23）：G-5c 为source bucket建立独立可复现的performance workload，而不是复用混有std/linker的`emit_exe`或完全不进LLVM的`module_backend`。新增单source、8个可达definition、只emit objects的`codegen_buckets`；runner强制要求`llvm.units >= 2`，否则拒绝样本，避免趋势数据静默退化为单unit或link测量。当前提交与父提交`e7aa1703`均以`perf-alloc` release compiler在同机同source各跑3次：1→4 units后CPU utilization中位114.49%→140.78%（约+23%），max RSS 58,458,112→60,002,304 bytes（约+2.64%），Rust allocator peak-live 1,237,801→1,241,304 bytes（约+0.28%），累计allocated约+1.68%；4 permits固定有2次memory wait。并行度与内存acceptance获得首个直接证据，但极小workload wall中位17.22→20.53 ms（约+19%），表明context/全程序declaration固定成本仍显著，不能由CPU utilization上升推导吞吐收益。Python perf/comparator 12项与真实4-object smoke通过。Phase G约94%，整份roadmap仍约93%；下一步应在代表性large codegen workload上比较wall/CPU/RSS并审计每bucket重复declaration成本，再决定阈值/上限，之后才推进frontend/LLVM overlap或关闭Phase G。

进展（2026-07-23）：G-5d 增加低frontend噪声的代表性large codegen workload：单source包含16个可达1 MiB static definitions而只有1个小function body，runner要求真实编译4个LLVM units且拒绝cache-hit样本。当前4-unit实现与拆分前1-unit编译器以同一`perf-alloc` release source交错各跑5次，中位wall 250.36→204.47 ms（约-18.33%）、`emit_native_objects` 197.72→151.53 ms（约-23.36%）、CPU utilization 121.67%→181.54%（约+49.21%），累计Rust allocation只增约0.016%；同时max RSS 124,559,360→210,530,304 bytes（约+69.02%），Rust peak-live 35,279,740→77,239,731 bytes（约+118.94%），4 permits中固定2次等待。LLVM IR审计确认16个global和1个function各只定义一次，但4 modules合计重复48个external-global与3个function declaration，即每bucket完整重建17-symbol declaration面。临时2-bucket对照中位wall 192.11 ms、CPU 177.82%、RSS 141,504,512 bytes、peak-live 68,839,151 bytes，证明当前8 GiB资源模型只允许2个并发LLVM task时，4个executor jobs的第二波没有吞吐收益并会把更多worker arena推到高水位；但直接把work products降为2会损失1/4局部invalidation粒度。下一切片因此保留4个stable CGU/object identities，只按LLVM memory capacity把它们编成有界worker lanes，让每个lane顺序消费多个partition，再重新量化RSS与wall。Python perf/comparator 14项通过。Phase G约95%，整份roadmap仍约93%。

进展（2026-07-23）：G-5e 将stable CGU粒度与实际executor worker数解耦。`QuerySession::run_tasks_bounded`把任意non-Clone tasks按round-robin分入不超过显式limit和session CPU budget的lane，每个lane顺序消费多个task，最终按原submission position归并；LLVM IR/native两条路径以正式memory task capacity为limit，4个partition/key/fingerprint/object/link input仍完全独立，不把资源调度反向写入持久identity。timing新增`llvm.worker_lanes`，large perf合同同时验证unit、lane和真实permit。当前8 GiB/32 CPU环境保持4 units但只使用2 lanes，同一workload 5次中位wall 198.81 ms、`emit_native_objects` 143.86 ms、CPU 184.02%、RSS 143,810,560 bytes、累计allocation 659,599,867 bytes、peak-live 77,246,633 bytes，4 permits且0 wait；相对G-5d旧4-worker调度RSS约-31.69%、wall约-2.77%，相对拆分前1 unit则wall约-20.59%、emit约-27.24%、CPU约+51.25%、RSS仅约+15.45%。worker arena高水位问题已消除，同时保留1/4局部object invalidation粒度，Phase G的CPU/RSS acceptance关闭；剩余主项是frontend与LLVM真实overlap。query 69项、LLVM 185项和Python perf/comparator 15项通过。Phase G约97%，整份roadmap约94%。

审计（2026-07-23）：frontend/LLVM overlap当前没有可由外层异步安全绕过的切点。`CodegenProgramQuery`同步等待完整`BackendLoweringQuery`；每个module finalization仍会执行devirtualization、cross-function propagation、inlining、DCE、aggregate/instance completion与layout扩张，因此未finalized module的ABI/layout/declaration surface尚未冻结。随后`ProgramIndex`一次索引全部final modules，`ModuleCodegen`为每个partition声明全程序全部aggregate/function/global/vtable，CGU declaration fingerprint也独立编码同一完整surface；任一module未完成时提前LLVM会同时缺失emission truth和cache identity。现有`nia-backend-lower::function_refs`只是reachability私有遍历，刻意不记录普通global refs，`StaticInit::refs`又不保留generic function instance identity，二者都不能直接冒充exact codegen dependency closure。无双轨迁移顺序因此固定为：先建立唯一typed Backend IR value-ref traversal，覆盖function/global、两类concrete instance与vtable method；再让partition declaration membership成为LLVM emission和version-bumped declaration fingerprint的共同唯一输入，物理删除两处全程序遍历；最后才把module finalization结果按真实dependency module发布给LLVM task，并删除aggregate `BackendLowering`等待路径。第一步本身可直接减少G-5d量化的重复declaration与过宽ABI invalidation；在这三步完成前不引入保守declaration副本、旧fingerprint兼容reader、side channel或仅改变线程形状的伪overlap。Phase G比例不因设计审计上调。

进展（2026-07-23）：G-6a 建立Function IR唯一typed value-reference traversal。`nia-function-ir::FunctionBody::value_refs`现在统一返回普通function/global、concrete function/global instance及其完整typed key和source span，递归覆盖block/terminator、defer、call callee与operands、inline asm input/output place、atomic和嵌套place index；普通global expression与place不再被reachability专用策略静默丢弃。原`nia-backend-lower::function_refs`中的约400行Function IR重复遍历、私有ref/key类型和optional-body collector已物理删除，backend closure与DCE直接消费IR-owned traversal；backend本地文件只保留`StaticInit` adapter，因为它必须把带type args的`AddrOfFunction`保留为concrete instance identity，同时也写入普通global集合。vtable方法继续在backend owner处投影进同一typed ref类型，没有第二套body visitor或兼容re-export。回归覆盖ordinary refs、两类instance、嵌套defer/call/atomic/asm/place及span-independent key去重；function-ir 7项、backend-lower 100项和两crate严格all-target/all-feature Clippy通过。下一切片让partition exact declaration membership成为LLVM declaration emission与version-bumped fingerprint的共同唯一输入，再删除whole-program declaration traversal；Phase G仍约97%。

进展（2026-07-23）：G-6b 补齐exact declaration closure的非symbol依赖，且没有在codegen复制IR visitor。`FunctionBodyRefs`现在从同一次递归收集body/local/expression/place/memory/atomic/callee内的全部typed type handles，并以typed `TraitObjectVtableRef { self_ty, object_ty }`记录具体trait-object coercion所需vtable；function/global instance的self/type/const-arg types与static function-address type args也进入同一集合。upcast只记录其source/target type而不虚构新vtable symbol，dynamic dispatch只记录ABI与object metadata types，保持实际LLVM语义。嵌套call→atomic operand、asm output place、defer instance、ordinary/global instance及trait-object coercion由同一回归共同锁定，function-ir 7项、backend-lower 100项与严格Clippy通过。下一步membership builder只允许读取`FunctionBodyRefs`、`StaticInit` adapter结果和final backend item/type index，不再遍历Function IR；Phase G仍约97%。

进展（2026-07-23）：G-6c 将static initializer也收敛到唯一typed value-ref traversal。`StaticInit::value_refs(ModuleId)`现在保留generic `AddrOfFunction`的concrete instance identity与type args，ordinary function/global继续进入同一`FunctionBodyRefs`；既有frontend轻量`StaticInitRefs`和新typed产品由一个私有sink visitor投影，zero-count repeat在两条语义上都不产生引用。backend closure、global-instance materialization和DCE全部直接调用IR-owned方法，`nia-backend-lower::function_refs`文件与module已物理删除；LLVM后续无需依赖backend-lower或复制static递归。static-ir 3项、backend-lower 100项与两crate严格Clippy通过。下一切片正式构造partition declaration membership；Phase G仍约97%。

进展（2026-07-23）：G-6d 建立partition exact declaration membership并删除LLVM whole-program declaration双轨。每个CGU从自身五类definition出发，只通过唯一`FunctionBody::value_refs(&TypeStore)`、`StaticInit::value_refs(ModuleId)`、vtable definition method与recursive `TyKind::visit_referenced_types`闭包引入真实function/global、concrete instance、vtable及nested nominal aggregate；referenced declaration只加入signature/layout，不递归foreign body/init。`ModuleCodegen`的aggregate/function/global/vtable声明与`source-declarations.v3` fingerprint共同消费这一membership，`ProgramIndex`原whole-program iterator API已物理删除，旧v2 fingerprint不读取、不fallback。跨partition非definition symbol统一显式external linkage；declaration encoder只记录LLVM ABI可见的passing type/return/variadic/attribute等字段，不再因param local id/name/receiver/local type变化过宽失效，target layout固定取partition owner而非program首模块，membership按stable source/type identity排序。审计large workload时发现array `.len()` receiver不求值却被typed traversal误记为global ref；唯一`value_refs`因此显式接收canonical `TypeStore`并按LLVM求值语义排除array receiver，同时保留slice receiver，backend reachability、DCE与LLVM全部迁移且没有旧无参入口。与G-5d同形状的16-static/4-CGU IR从48个重复external-global加3个重复function declaration降为0，16个global与1个function仍各定义一次；多bucket回归锁定仅main依赖bucket产生5个真实function declaration，未引用ABI变化只失效1个CGU，跨bucket真实ABI变化只失效定义与依赖2个CGU。Function IR 8项、backend-lower 100项、LLVM 189项、Driver 488项、跨进程object/link cache回归及三crate严格Clippy/fmt/diff check全部通过。Phase G约98%，整份roadmap约95%；下一切片只能把final backend module按已冻结dependency membership发布给LLVM task并删除aggregate `BackendLowering`等待路径，随后以真实timeline与clean-recompute差分关闭frontend/LLVM overlap，不得恢复保守全程序声明、旧fingerprint reader或side-channel membership。

进展（2026-07-23）：G-6e1 将module finalization从“executor并行、整批等待后才交付”推进为production完成流。`QueryBatch`现在在同一锁快照内记录completion queue与remaining状态，新的`QueryDb::for_each_many_owned`在调用线程按真实完成顺序move单消费者结果，同时复用既有persistent executor、jobserver budget、nested query stack和dependency/fingerprint合并；最后一个worker恰好落在drain与完成检查之间的竞态由三层回归实际捕获并消除。compiler backend provider不再调用`get_many_owned`形成整批`Vec<BackendModuleFinalization>`，而是让唯一`BackendModuleFinalizationCollector`逐项吸收完成结果、按position验证owner并最终确定性合并module/diagnostic/optimization report；旧`finish_backend_module_finalizations`批量入口已物理删除，串行backend路径也使用同一collector。完成顺序、non-Clone move、parent dependency invalidation、资源预算和反序确定性均有回归；query 71项、backend-lower 100项、compiler-query 146项与三crate严格Clippy通过。该切片建立真实“完成即交付”边界，但LLVM尚无consumer，不能宣称overlap完成；Phase G仍约98%，整份roadmap仍约95%。下一切片必须让交付事件携带可验证的final module dependency owner product并进入driver-owned LLVM编排，不能以callback side channel、全程序保守等待或per-module `Arc`副本冒充最终所有权方案。

进展（2026-07-23）：G-6e2 修复并固化重复 trait-object vtable 的唯一 owner。真实三模块 workload 证明相同 `(self_ty, object_ty)` vtable 过去会在两个 source module 各自进入 definition plan，而 LLVM symbol 只由这两个 ABI 类型决定；`ProgramIndex` 的覆盖式 hashmap 插入会掩盖重复 owner，最终可能产生重复 definition 或错误 entry 来源。backend closure 完成后、module item plan 发布前新增唯一 owner pass：按 normalized source identity 选择稳定 owner，要求重复 key 的 trait/args/entry payload 完全一致，并从非 owner module 物理删除 vtable；`CodegenPartitionPlan` 同时拒绝任何绕过 lowerer 的重复 vtable definition。重复 vtable regression、backend-ir 10项、backend-lower 100项、compiler-query 147项与LLVM 189项通过，既有Driver全量488项通过；没有给 vtable key 添加伪 module id，也没有保守 external fallback。Phase G仍约98%，整份roadmap仍约95%；下一切片才能在这个唯一 owner product之上建立 final module dependency closure并接入 driver-owned LLVM 编排。

进展（2026-07-24）：G-6e3 建立与 `CodegenUnitId` 强绑定的 typed final-module dependency closure。`CodegenUnitDependencies` 只保存排序去重后的 `ModuleId` 前置条件，不持有或复制module payload；partition exact declaration builder从definition owner出发，统一记录普通function/global、concrete function/global/aggregate instance的实际发布位置、G-6e2唯一vtable owner、nominal/enum语义owner、source trait/associated binding/projection owner及const-array-length owner，不以`def_id.module_id`猜测concrete instance位置。fingerprint与LLVM emission在各自入口共同验证同一closure的unit identity、definition owner和全部已发布module，未建立第二membership或fallback truth source。回归锁定self-contained unit只含owner、foreign function引用精确加入实际owner、unrelated module不进入closure，并证明semantic def owner与actual instance/vtable发布module不同时选择后者；backend-ir 12项、LLVM 191项、compiler-query相关严格Clippy、fmt与diff check通过。该产品目前仍在完整`ProgramIndex`构造之后消费，driver尚未按module readiness启动LLVM，且whole-program backend validation仍位于task启动前，因此frontend/LLVM overlap尚未完成，Phase G与整体完成度不虚增；下一切片应把validation收窄为dependency-scoped product，再让finalization completion事件驱动driver-owned LLVM orchestration并删除aggregate `BackendLowering`等待路径。

进展（2026-07-24）：G-6e4 删除LLVM task启动前的whole-program backend validation barrier。验证现在拆成唯一两段unit contract：membership构造前只验证当前partition拥有的function/global、两类concrete instance与vtable definitions，保证missing ref仍产生结构化diagnostic而不会先在closure构造中panic；closure建立后只验证其exact function/global/aggregate/vtable declarations及ABI types，不扫描unrelated module或foreign body/init。每个task以diagnostic vector返回全部本unit错误，LLVM context与memory permit均在definition/declaration validation和fingerprint之后才创建/获取；旧`validate_backend_program`与完整module body traversal已物理删除。没有CGU的纯declaration program使用明确`DeclarationModule` validation task保持validator API对坏ABI输入不静默成功，它不与真实CGU路径形成fallback或全局等待。原14项malformed Backend IR诊断回归、LLVM 191项、compiler-query 147项、Driver 488项、严格Clippy、fmt与diff check通过。当前仍由完整`BackendLowering`一次构造`ProgramIndex`，module completion尚未直接发布partial index/readiness并驱动LLVM，因此Phase G与整体完成度仍不虚增；下一切片应把finalization completion、dependency readiness与partition launch收敛进driver-owned coordinator，最终删除aggregate lowering wait path。

进展（2026-07-24）：G-6e5a 为completion-driven coordinator建立唯一ID-owned final module arena。新增`BackendModuleStore`，在finalization启动前按确定序预注册全部`ModuleId`，每个slot以`OnceLock<BackendModule>`接受恰好一次并发发布；reader只持共享arena与`ModuleId`并借用已ready payload，没有per-module `Arc`、module clone、callback side store或第二truth source。`BackendModuleFinalizationCollector`已从`Vec<Option<BackendModule>>`迁到该arena，completion按真实顺序直接发布，外部可在aggregate finish前持有同一arena观察module readiness；所有reader释放后collector以`Arc::try_unwrap`无复制恢复原声明顺序的`BackendProgram`，重复owner、未注册owner、重复publish、缺失slot和aggregate finish仍有明确ICE。并发反序发布、live reader及确定序还原回归通过，backend-ir 15项、backend-lower 100项、compiler-query 147项与严格Clippy/fmt/diff check通过。该切片只完成payload ownership前置，不宣称LLVM已与frontend重叠；下一切片应让finalization schedule正式返回共享arena和typed completion/readiness事件，再把`ProgramIndex`改为按ready module增量发布。

进展（2026-07-24）：G-6e5b 在唯一module arena上建立正式single-consumer typed readiness stream。每次slot首次发布后生成不可伪造的`BackendModuleReady { position, module_id }`，`BackendModuleReadiness::wait_next`按真实publication lock顺序交付；completion queue与“全部预注册slot均已发布”的终止判断读取同一mutex快照，最后一个publisher不能落在drain/complete检查之间而丢失事件。arena以atomic claim拒绝第二consumer，receiver持有的只是同一arena控制块和cursor；事件不携带payload、不复制module，也没有polling map或callback channel。collector正式暴露唯一receiver，回归在反序finalization中同时验证`second → first → None`、live store借用和释放reader后无复制aggregate finish；backend-ir 17项、backend-lower 100项及三crate严格Clippy/fmt/diff check通过。typed stream现已具备，但compiler provider仍在`BackendLoweringQuery`内部同步drain后才返回；下一切片必须让schedule/stream越过该aggregate query边界，并让incremental `ProgramIndex`按ready module发布，仍不能把事件仅记录成日志后继续整体等待。

进展（2026-07-24）：G-6e5c 消除LLVM `ProgramIndex` position对`BackendProgram.modules` Vec ordinal的结构依赖。item、layout与enum variant三类position现在直接保存实际发布owner的`ModuleId`，owner查询不再从下标反推identity，所有payload读取统一经过唯一`module_at(ModuleId)` resolver；semantic def owner与concrete instance/vtable实际发布owner不同的回归继续锁定后者，并新增layout、function instance与enum variant内部position断言，防止未来退回输入顺序身份。LLVM 191项、定向回归、严格Clippy、fmt与diff check通过。当前resolver仍通过`Arc<BackendLowering>`持有完整aggregate和`ModuleId → Vec index`映射，尚未消费`BackendModuleStore`，因此不宣称已有incremental index或frontend/LLVM overlap；下一切片必须把`ProgramIndex`的payload owner迁到唯一module arena，并让lookup table只由ready module增量发布，物理删除完整program构造入口与双轨索引路径。

进展（2026-07-24）：G-6e5d 将final backend module的canonical ownership永久留在`BackendModuleStore`。`BackendProgram.modules`不再是第二个`Vec<BackendModule>` payload owner，而是同一arena上的确定序只读`BackendModules`视图；collector aggregate finish已物理删除`Arc::try_unwrap`/`into_program`搬移路径，store与readiness reader可以跨finish继续存活，指针相等回归证明aggregate、reader与后续consumer观察同一allocation。LLVM `ProgramIndex`同时删除`Arc<BackendLowering>`和`ModuleId → Vec ordinal`映射，只持共享arena与`TypeStore`，module/partition/builtin/declaration/fingerprint/codegen读取全部经ID resolver落回唯一slot；没有per-module `Arc`、module clone、兼容Vec owner或fallback lookup。backend-ir 17项、backend-lower 100项、LLVM 191项、compiler-query 147项、workspace all-target/all-feature check、四crate严格Clippy、fmt与diff check通过。当前`ProgramIndex::new`仍要求所有slot已发布并一次性构造全部lookup tables，因此frontend/LLVM仍未重叠；下一切片必须把tables改为single-writer按`BackendModuleReady`增量发布，并以unit dependency全部ready作为唯一task launch条件，删除full-index build barrier而不保留双轨入口。

进展（2026-07-24）：G-6e5e 建立LLVM lookup tables的唯一增量发布路径。`ProgramIndex::new`现在只创建共享arena/type-store reader与空tables，并同时返回不可clone的唯一`ProgramIndexPublisher`；每个ready `ModuleId`必须经publisher在单次write-lock临界区原子加入全部item/layout/enum/vtable position，重复发布明确ICE，reader只能观察“未索引”或“完整索引”的module。全部lookup accessor已迁到并发只读tables，返回payload前复制的只是轻量ID position；by-def/vtable/layout iterator持有position snapshot而不持锁、不复制Backend IR。`ProgramIndex::module`也只暴露已完成table publication的slot，不能把arena readiness绕过成index readiness。现有IR/native/fingerprint路径已删除full-program constructor并逐module调用同一publisher，没有batch alias或第二索引实现；反序发布回归验证第二module可先独立查询、第一module仍不可见，并在reader循环并发时发布第一module，最终owner identity保持各自`ModuleId`。LLVM 192项、compiler-query 147项、workspace all-target/all-feature check、严格Clippy、fmt与diff check通过。当前完整`BackendLoweringQuery`仍先返回，两个codegen入口也仍在fan-out前把全部module送过publisher，因此不宣称真实overlap；下一切片必须让finalization schedule、arena与readiness receiver越过aggregate query边界，并由driver-owned coordinator执行`ready event → index publish → dependency readiness → partition launch`，删除启动前全量publication loop。

进展（2026-07-24）：G-6e5f 将query completion从回调形状收敛为typed pull stream。`QueryDb::with_many_owned_completion`在既有persistent executor、activity guard、nested query stack、jobserver budget和dependency/fingerprint合并语义内交付唯一借用式`QueryCompletionStream::wait_next()`；consumer主动按真实完成顺序move non-Clone值，提前返回或panic时框架仍drain全部worker，旧`run_tasks_inner_with_completion`实现已物理删除，`for_each_many_owned`也只是同一stream的普通drain consumer。compiler backend provider新增唯一`BackendFinalizationSchedule`，每次`wait_next`严格执行`query completion → collector publish → BackendModuleReady`并校验completion/readiness position一致，aggregate finish也通过该schedule drain，不再以callback直接写collector。query 72项、compiler-query 147项及两crate严格Clippy、fmt与diff check通过；completion order、父查询依赖和既有executor budget回归保持。当前schedule仍在`BackendLoweringQuery` provider内部完整drain后返回，尚未越过aggregate边界，不能宣称Driver coordinator或overlap已完成；下一切片必须让pre-backend codegen product与该typed schedule进入唯一Driver编排入口，同时删除Driver先调用完整`codegen()`再启动LLVM的barrier路径。

进展（2026-07-24）：G-6e5g 冻结partial dependency readiness所需的typed actual-owner directory。`BackendModuleOwnerDirectory`在`BackendItemPlan`已经完成跨模块closure、concrete instance分配与唯一vtable owner之后构造，分别记录普通item、struct/union/global/function concrete instance和vtable key的实际发布`ModuleId`，只保存typed key与ID，不持有或复制Backend IR；它被嵌入不可clone的`BackendItemPlanFinalization`并随唯一collector/schedule移动，不是side store。`BackendFinalizationSchedule`已提升为compiler-query正式公开类型，唯一暴露同一arena、owner directory、`wait_next`与无复制aggregate `finish`；回归锁定semantic def owner与actual global-instance publication owner不同时必须选择后者。backend-ir 18项、backend-lower 100项、compiler-query 147项及三crate严格Clippy/fmt/diff check通过。partial `CodegenDeclarationMembership`目前遇到未发布owner仍会panic，schedule也仍由aggregate provider内部构造并drain，因此Driver barrier尚未删除；下一切片必须让membership读取该directory并返回typed `Pending(ModuleId set)`/`Ready`，随后coordinator才能只等待exact owners而不是退回全模块保守等待。

进展（2026-07-24）：G-6e5h 将codegen declaration closure的“尚未发布”从panic迁移为唯一typed构建结果。`CodegenDeclarationMembership::build`现在只能返回`Ready(CodegenDeclarationMembership)`或`Pending(CodegenUnitPendingModules)`；pending product强绑定`CodegenUnitId`并持有排序去重的非空`ModuleId`集合，不持有module payload。普通function/global、concrete function/global/aggregate instance、vtable及其entry均只通过G-6e5g actual-owner directory选择等待对象；nominal struct/union instance优先选择concrete actual owner，enum与trait/const/type语义依赖也只等待各自精确owner。directory缺owner仍是结构错误，owner已经发布但`ProgramIndex`仍缺payload也继续ICE，不能被降级成pending或全模块fallback。唯一directory在aggregate lowering中整体进入一个共享`Arc`供LLVM task读取，不复制entry、Backend IR或per-module owner；现有完整程序IR/native入口显式拒绝pending，未保留旧builder或ready快捷双轨。回归证明semantic def owner与actual function-instance发布owner不同时pending集合只含actual owner，发布后同一unit重试为ready且dependency closure只含caller与actual owner，并锁定published-owner/missing-payload仍panic；backend-ir 18项、backend-lower 100项、LLVM 194项、compiler-query 147项通过。Driver目前仍先取得完整`BackendLowering`并全量publish index，因此本切片只关闭coordinator所需的typed retry协议，不宣称frontend/LLVM overlap已经完成，Phase G与整体完成度不虚增；下一切片必须让唯一coordinator消费`BackendFinalizationSchedule`，执行`BackendModuleReady → ProgramIndexPublisher::publish → pending membership retry → exact dependency ready → partition launch`，并删除启动前全量publication loop。

进展（2026-07-24）：G-6e5i 让live backend schedule正式越过aggregate query边界而不伪造可逃逸生命周期。新增tracked `CodegenPreparation` product，只发布type store、graph、optimization、checked-module句柄、monomorphization句柄和pre-backend diagnostics；`CompilerDatabase::codegen_preparation`在同一provider-demand worklist收敛协议中取得它，但不执行`BackendLoweringQuery`或任何module finalization。`CompilerDatabase::with_backend_finalization_schedule`以higher-ranked scoped consumer交付唯一borrowed schedule，completion stream不能被保存到callback之外；diagnostic plan明确走`Err(BackendLowering)`而不会启动伪schedule。原aggregate `BackendLoweringQuery`已改为消费同一scoped helper并调用`finish`，不存在第二套plan publication/finalization路径。两模块回归锁定preparation trace中backend lowering/finalization执行数均为0，并证明第一次`wait_next`后canonical store只发布该ready module、另一module直到后续drain仍不可见，同时整个scoped路径不执行aggregate query；compiler-query 149项通过。该切片只建立Driver/LLVM coordinator可进入的真实调度边界，尚未在callback中启动LLVM task，因此Phase G与整体完成度仍不虚增；下一切片直接在该scope内创建唯一incremental `ProgramIndex`，消费readiness并按typed pending集合重试partition，随后接入Driver emission入口并删除其`codegen() → LLVM` barrier。

进展（2026-07-24）：G-6e5j 深度审计否决了“item closure收敛后即可冻结最终CGU plan”的假设。完整LLVM回归证明现有module finalization仍会物化pre-finalization `BackendModuleItemPlan`中不存在的function与aggregate；直接提前计算plan会漏definition，G-6e5g owner directory也无法为这些尚未物化的普通item提供owner，partial membership因而会把健康跨模块nominal引用误判为missing owner。实验性pre-finalization `CodegenPartitionPlan` API、schedule字段和post-finalization equality断言已全部物理删除，未保留provisional/final双轨或fallback；现有完整程序LLVM路径继续在全部canonical modules发布后构造唯一最终plan并保持validator先于membership。这个结果不回退G-6e5h对已登记actual owner的typed pending语义，但说明它尚不能覆盖全部production dependency closure，也不能据此启动LLVM。下一切片必须先把finalization-created item的definition/owner planning前移成typed manifest，或把module finalization拆成“完整membership冻结 → payload优化发布”两个正式阶段；在该缺口关闭前不得接入readiness coordinator，也不得用`def_id.module_id`猜owner。

进展（2026-07-24）：G-6e5k 完成finalization membership调用图审计并关闭G-6e5j blocker。此前pre/final CGU差异中的function变化被确认是module DCE删除definition，而不是finalization创建function；真正的新增路径只有`extend_struct_instances_from_functions`与`complete_reachable_aggregates`。两者现已从`finish_module`物理前移到跨模块item closure之后的唯一`backend_lower.definition_membership`阶段，复用当次lowerer已经建立的aggregate source index，不再由finalization重建source map；由此唯一`BackendModuleOwnerDirectory`在schedule创建前已经覆盖普通aggregate与concrete aggregate definition。前移同时暴露并修复了旧finalization曾隐藏的跨模块concrete struct/union instance重复发布：相同typed key的payload必须完全一致，actual owner按stable source identity唯一选择，非owner副本被物理删除，不再依赖`ProgramIndex`覆盖插入。每个finalized module发布前必须通过manifest逐类验证，任何普通item、四类instance或vtable在优化阶段凭空出现都会ICE；DCE仍可合法删除function/function-instance，因此没有再次伪造optimization-independent最终CGU plan。跨模块struct literal回归在O0/O1/O2/O3/Os/Oz六种策略下均锁定“owner manifest已发布而geom payload尚未ready”，并验证最终payload兑现相同nominal definition；backend-ir 18项、backend-lower 100项、LLVM 194项及定向compiler-query回归通过。下一切片应从每个ready canonical module构造其唯一final partition集合并立即进入typed membership retry，最终plan只作为已发布module partition的确定序汇总，不再阻塞首批LLVM task。

进展（2026-07-24）：G-6e5l 建立ready module到最终CGU partition的唯一构造路径。`CodegenPartition`已物理删除`BackendProgram.modules` Vec ordinal，只保留stable `CodegenUnitId/CodegenUnitKey`与definition positions；完整程序和增量`ProgramIndex`均从unit中的真实`ModuleId`解析同一canonical store payload。`CodegenPartitionPlan::for_ready_module`现在可在单个module publication后立即按最终优化payload生成definition-filtered buckets，完整程序plan复用同一个module iterator实现做stable-key排序与全局唯一性校验，不存在provisional partition格式或第二套bucket算法。回归证明反序存放的program中，单个ready module plan与最终stable全局plan的精确对应切片相等；backend-ir 18项、LLVM 194项通过。下一切片可由Driver coordinator执行`wait_next → ProgramIndexPublisher::publish → for_ready_module → membership Ready/Pending`，不再等待aggregate lowering或依赖module Vec位置。

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

Nia 当前不是“实现质量落后一大截、需要全面重写”，也不是“只差几个热点优化”。它已经具备第一梯队编译器常见的许多组件：lossless syntax、typed query、trait/normalize/const-eval 分层、function IR、reachability、monomorphization、LLVM backend、丰富测试。真正的问题是这些组件之间缺少一个统一、性能导向的 compiler kernel。

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
