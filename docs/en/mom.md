# Machina Object Model (MOM)

## 1. Scope

This document describes Machina's first-cut device model alignment with
QEMU's object/qdev/sysbus direction, using Machina-native terminology:
`MOM`, `mobject`, `mdev`, and `sysbus`.

This is a direct replacement of the previous thin qdev/sysbus skeleton.
There is no compatibility or migration layer beyond the temporary qdev bridge
already used inside the codebase.

The current first-cut MOM scope covers:

- the root object layer (`mobject`)
- the device layer (`mdev`)
- executable sysbus realization and unrealize
- a lightweight property surface
- migrated platform devices: UART, PLIC, ACLINT, and virtio-mmio

## 2. Layering

### 2.1 `mobject`

`mobject` is the foundational ownership and identity layer.

- It lives in `machina-core`
- It gives managed objects a local ID and object path
- It enforces a strict parent/child tree
- It is the reason `Machine` now participates in the object tree

### 2.2 `mdev`

`mdev` is the common device lifecycle layer on top of `mobject`.

- It lives in `machina-hw-core`
- It tracks `realize` / `unrealize`
- It rejects forbidden late structural mutation
- It carries the common error taxonomy for migrated devices

### 2.3 `sysbus`

`sysbus` is an executable assembly layer, not metadata only.

- Devices must attach to a bus before realization
- Devices must register MMIO regions before realization
- Realization validates overlaps and maps regions into `AddressSpace`
- Unrealize removes realized mappings from `AddressSpace` and the bus record

### 2.4 Properties

The first MOM increment uses a small typed property layer.

- Property schema is defined before realization
- Required/default handling is explicit
- Static-vs-dynamic mutability is explicit
- UART uses a standard `chardev` link property on this surface

## 3. Device Lifecycle

The migrated-device lifecycle is:

1. Create the device object
2. Attach to `sysbus`
3. Register MMIO and any device-specific runtime wiring inputs
4. Apply pre-realize properties
5. Realize onto `AddressSpace`
6. Reset runtime state without rebuilding topology
7. Unrealize by tearing down runtime state and removing realized mappings

The key rule is that structural topology is created once and then preserved
across reset. Reset must not rebuild hidden topology as a side effect.

## 4. First-Cut Migrated Devices

### 4.1 UART

- Owns a `SysBusDeviceState`
- Exposes `chardev` as a standard property
- Installs frontend runtime wiring during `realize`
- Removes runtime wiring and MMIO mapping during `unrealize`

### 4.2 PLIC

- Owns a `SysBusDeviceState`
- Keeps context-output routing as device-specific runtime wiring
- Uses runtime reset without rebuilding sysbus topology

### 4.3 ACLINT

- Owns a `SysBusDeviceState`
- Keeps MTI/MSI and WFI-waker wiring device-specific
- Cancels timer state on reset and unrealize without rebuilding topology

### 4.4 virtio-mmio

- The MMIO transport is the MOM/sysbus device
- The block backend remains transport-local
- The transport owns guest-RAM access, MMIO state, and IRQ delivery

This keeps the transport/proxy boundary explicit and leaves room for future
backend relationships without conflating them with machine assembly.

## 5. `RefMachine` Assembly Rule

`RefMachine` is the first machine that follows the MOM assembly rule for the
migrated set.

- UART, PLIC, ACLINT, and virtio-mmio are created as MOM-managed devices
- They are attached and realized through `sysbus`
- Their realized mappings are visible through `SysBus::mappings()`
- FDT node names and `reg` cells for the migrated set are derived from the
  realized sysbus mappings

For the migrated device set, realized `sysbus` mappings are the machine-side
topology source of truth.

## 6. Testing and Regression Guardrails

The shared `tests` crate verifies:

- object attachment and lifecycle sequencing
- MMIO visibility only after realization
- UART, PLIC, ACLINT, and virtio-mmio guest-visible behavior
- sysbus unrealize/unmap behavior
- machine-visible migrated owner sets
- source-level anti-regression checks against direct root MMIO wiring

## 7. Future Extension Points

The current design intentionally leaves explicit extension points for:

- PCI and non-sysbus transports
- hotplug-aware lifecycle extensions
- richer object/property introspection
- parent/child relationships between transport devices and backend devices

These are future directions, not v1 commitments.
