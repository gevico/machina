# Machina Object Model（MOM）

## 1. 范围

本文档说明 Machina 当前第一阶段设备模型如何对齐 QEMU 的
object/qdev/sysbus 方向，并统一使用 Machina 自身术语：
`MOM`、`mobject`、`mdev`、`sysbus`。

这一轮是对旧薄壳 qdev/sysbus 结构的直接替换。除代码里暂时保留的
qdev bridge 外，不再提供额外兼容层或迁移层。

当前第一阶段 MOM 覆盖范围包括：

- 根对象层（`mobject`）
- 设备层（`mdev`）
- 可执行的 sysbus realize / unrealize
- 轻量属性表面
- 已迁移的平台设备：UART、PLIC、ACLINT、virtio-mmio

## 2. 分层

### 2.1 `mobject`

`mobject` 是所有权和身份标识的基础层。

- 位于 `machina-core`
- 为受管对象提供 local ID 和 object path
- 强制父子严格树结构
- 也是 `Machine` 进入对象树的基础

### 2.2 `mdev`

`mdev` 是建立在 `mobject` 之上的公共设备生命周期层。

- 位于 `machina-hw-core`
- 负责跟踪 `realize` / `unrealize`
- 拒绝非法的 realize 后结构性修改
- 为已迁移设备提供统一错误语义

### 2.3 `sysbus`

`sysbus` 是可执行装配层，而不只是元数据。

- 设备在 realize 前必须先 attach 到 bus
- 设备在 realize 前必须先注册 MMIO region
- realize 会校验重叠并把 region 映射进 `AddressSpace`
- unrealize 会把已实现映射从 `AddressSpace` 和 bus 记录里移除

### 2.4 属性

当前 MOM 第一阶段使用轻量、强类型的属性层。

- 属性 schema 在 realize 前定义
- required/default 语义显式化
- static / dynamic 可变性边界显式化
- UART 的 `chardev` 就通过这层作为标准属性暴露

## 3. 设备生命周期

已迁移设备的生命周期为：

1. 创建设备对象
2. attach 到 `sysbus`
3. 注册 MMIO 和设备特定运行时接线输入
4. 应用 realize 前属性
5. realize 到 `AddressSpace`
6. reset 仅重置运行时状态，不重建拓扑
7. unrealize 时先拆运行时状态，再移除已实现映射

核心规则是：结构性拓扑只创建一次，并跨 reset 保持稳定。reset 不能
隐式重建拓扑。

## 4. 第一阶段已迁移设备

### 4.1 UART

- 持有 `SysBusDeviceState`
- 通过标准属性暴露 `chardev`
- 在 `realize` 时安装 frontend 运行时接线
- 在 `unrealize` 时同时拆运行时接线和 MMIO 映射

### 4.2 PLIC

- 持有 `SysBusDeviceState`
- context output 路由仍保持为设备特定运行时接线
- reset 仅重置运行时状态，不重建 sysbus 拓扑

### 4.3 ACLINT

- 持有 `SysBusDeviceState`
- MTI/MSI 和 WFI-waker 保持为设备特定运行时接线
- reset / unrealize 时清理定时器状态，但不重建拓扑

### 4.4 virtio-mmio

- MOM/sysbus 设备是 MMIO transport 本身
- block backend 仍保持为 transport 内部关系
- transport 自己拥有 guest RAM 访问、MMIO 状态和 IRQ 传递

这样可以明确 transport/proxy 边界，并为后续更复杂的 backend 关系预留
扩展空间，而不会把它们和 machine assembly 混在一起。

## 5. `RefMachine` 装配规则

`RefMachine` 是第一台完整遵守 MOM 装配规则的机器。

- UART、PLIC、ACLINT、virtio-mmio 都作为 MOM 设备创建
- 它们统一通过 `sysbus` attach 和 realize
- realized mapping 统一通过 `SysBus::mappings()` 暴露
- migrated set 的 FDT node name 和 `reg` 字段从 realized sysbus mapping 派生

对于已迁移设备集合，realized `sysbus` mapping 就是 machine 侧拓扑的
单一事实来源。

## 6. 测试与防回退护栏

共享 `tests` crate 当前覆盖：

- 对象挂接和生命周期顺序
- MMIO 只有在 realize 后才可见
- UART、PLIC、ACLINT、virtio-mmio 的客户可见行为
- sysbus unrealize / unmap 行为
- machine 侧 migrated owner 集合
- 防止回退到 direct root MMIO wiring 的源码级检查

## 7. 未来扩展点

当前设计明确保留了以下扩展点：

- PCI 和非 sysbus transport
- 支持 hotplug 的生命周期扩展
- 更丰富的对象/属性 introspection
- transport device 和 backend device 之间更正式的父子关系

这些是未来扩展方向，不属于 v1 承诺。
