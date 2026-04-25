use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use machina_core::address::GPA;
use machina_hw_char::uart::Uart16550;
use machina_hw_core::bus::SysBus;
use machina_hw_core::chardev::{CharFrontend, Chardev};
use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_memory::address_space::AddressSpace;
use machina_memory::region::MemoryRegion;

// -- Test helpers --

struct TestIrqSink {
    levels: Vec<AtomicBool>,
}

impl TestIrqSink {
    fn new(n: usize) -> Self {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(AtomicBool::new(false));
        }
        Self { levels: v }
    }

    fn level(&self, irq: u32) -> bool {
        self.levels[irq as usize].load(Ordering::Relaxed)
    }
}

impl IrqSink for TestIrqSink {
    fn set_irq(&self, irq: u32, level: bool) {
        if let Some(f) = self.levels.get(irq as usize) {
            f.store(level, Ordering::Relaxed);
        }
    }
}

#[test]
fn test_uart_lsr_initial() {
    let uart = Uart16550::new();
    let lsr = uart.read(5);
    // THR empty (bit 5) and transmitter empty (bit 6).
    assert_ne!(lsr & 0x20, 0, "THRE should be set");
    assert_ne!(lsr & 0x40, 0, "TEMT should be set");
}

#[test]
fn test_uart_write_thr() {
    let uart = Uart16550::new();
    uart.write(0, 0x41); // write 'A'
    let lsr = uart.read(5);
    assert_ne!(lsr & 0x20, 0, "THRE should remain set");
}

#[test]
fn test_uart_receive() {
    let uart = Uart16550::new();
    uart.receive(0x42); // push 'B'
    let lsr = uart.read(5);
    assert_ne!(lsr & 0x01, 0, "DR should be set");
}

#[test]
fn test_uart_read_rbr() {
    let uart = Uart16550::new();
    uart.receive(0x42);
    let ch = uart.read(0);
    assert_eq!(ch, 0x42);

    // After reading, DR should be cleared (FIFO empty).
    let lsr = uart.read(5);
    assert_eq!(lsr & 0x01, 0, "DR should be cleared");
}

#[test]
fn test_uart_dlab() {
    let uart = Uart16550::new();

    // Set DLAB.
    uart.write(3, 0x80);

    // Write DLL and DLM.
    uart.write(0, 0x0C); // DLL = 12
    uart.write(1, 0x00); // DLM = 0

    // Read them back.
    assert_eq!(uart.read(0), 0x0C);
    assert_eq!(uart.read(1), 0x00);

    // Clear DLAB, verify normal register access.
    uart.write(3, 0x00);
    uart.write(1, 0x01); // IER = enable RX
    assert_eq!(uart.read(1), 0x01);
}

#[test]
fn test_uart_fifo() {
    let uart = Uart16550::new();

    // Push multiple bytes.
    uart.receive(0x61); // 'a'
    uart.receive(0x62); // 'b'
    uart.receive(0x63); // 'c'

    // Read them in order.
    assert_eq!(uart.read(0), 0x61);
    assert_eq!(uart.read(0), 0x62);
    assert_eq!(uart.read(0), 0x63);

    // FIFO empty now.
    let lsr = uart.read(5);
    assert_eq!(lsr & 0x01, 0, "DR should be cleared");
}

#[test]
fn test_uart_irq_on_receive() {
    let uart = Uart16550::new();

    // Enable RX available interrupt.
    uart.write(1, 0x01); // IER bit 0

    // No data yet, no IRQ.
    assert!(!uart.irq_pending());

    // Receive a byte -- should raise IRQ.
    uart.receive(0x55);
    assert!(uart.irq_pending());

    // IIR should indicate RX data available (0x04).
    let iir = uart.read(2);
    assert_eq!(iir, 0x04);

    // Read the byte -- IRQ should clear.
    let _ = uart.read(0);
    assert!(!uart.irq_pending());
}

#[test]
fn test_uart_tx_to_chardev() {
    let mut bus = SysBus::new("sysbus0");
    let mut address_space =
        AddressSpace::new(MemoryRegion::container("system", u64::MAX));
    let uart = Arc::new(Uart16550::new_named("uart0"));
    let buf_ref = Arc::new(Mutex::new(Vec::<u8>::new()));

    struct SharedChardev {
        buf: Arc<Mutex<Vec<u8>>>,
    }
    impl Chardev for SharedChardev {
        fn read(&mut self) -> Option<u8> {
            None
        }
        fn write(&mut self, data: u8) {
            self.buf.lock().unwrap().push(data);
        }
        fn can_read(&self) -> bool {
            false
        }
    }
    let shared_buf = Arc::clone(&buf_ref);
    let chardev = SharedChardev { buf: shared_buf };
    let fe = CharFrontend::new(Box::new(chardev));

    {
        uart.attach_to_bus(&mut bus).unwrap();
        let region = MemoryRegion::io(
            "uart0",
            0x100,
            Arc::new(machina_hw_char::uart::Uart16550Mmio(Arc::clone(&uart))),
        );
        uart.register_mmio(region, GPA::new(0x1000_0000)).unwrap();
        uart.attach_chardev(fe).unwrap();
        let rx_cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
            Arc::new(Mutex::new(move |_byte: u8| {}));
        uart.realize_onto(&mut bus, &mut address_space, rx_cb)
            .unwrap();
    }

    // Write 'A' to THR.
    uart.write(0, 0x41);
    let got = buf_ref.lock().unwrap().clone();
    assert_eq!(got, vec![0x41], "chardev should receive 'A'");

    // Write another byte.
    uart.write(0, 0x42);
    let got = buf_ref.lock().unwrap().clone();
    assert_eq!(got, vec![0x41, 0x42], "chardev should receive both bytes");
}

#[test]
fn test_uart_rx_irq_line() {
    let mut bus = SysBus::new("sysbus0");
    let mut address_space =
        AddressSpace::new(MemoryRegion::container("system", u64::MAX));
    let uart = Arc::new(Uart16550::new_named("uart0"));

    // Create test IRQ sink and attach line.
    let sink = Arc::new(TestIrqSink::new(16));
    let irq_num = 10u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, irq_num);
    {
        uart.attach_to_bus(&mut bus).unwrap();
        let region = MemoryRegion::io(
            "uart0",
            0x100,
            Arc::new(machina_hw_char::uart::Uart16550Mmio(Arc::clone(&uart))),
        );
        uart.register_mmio(region, GPA::new(0x1000_0000)).unwrap();
        uart.attach_irq(line).unwrap();
        let rx_cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
            Arc::new(Mutex::new(move |_byte: u8| {}));
        uart.realize_onto(&mut bus, &mut address_space, rx_cb)
            .unwrap();
    }

    // Enable RX available interrupt.
    uart.write(1, 0x01);

    // IRQ line should be low before data arrives.
    assert!(!sink.level(irq_num), "IRQ should be low before receive");

    // Receive a byte -- IRQ should assert.
    uart.receive(0x55);
    assert!(sink.level(irq_num), "IRQ should be raised after receive");

    // Read the byte -- IRQ should deassert.
    let _ = uart.read(0);
    assert!(
        !sink.level(irq_num),
        "IRQ should be lowered after reading RBR"
    );
}

#[test]
fn test_uart_chardev_property_set_and_get() {
    let uart = Uart16550::new_named("uart0");
    assert_eq!(uart.chardev_property(), None);
    uart.set_chardev_property("/machine/chardev/uart0").unwrap();
    assert_eq!(
        uart.chardev_property().as_deref(),
        Some("/machine/chardev/uart0")
    );
}

#[test]
fn test_uart_realize_via_sysbus_installs_runtime_wiring() {
    struct LoopbackChardev {
        tx: Arc<Mutex<Vec<u8>>>,
        startup_byte: u8,
    }

    impl Chardev for LoopbackChardev {
        fn read(&mut self) -> Option<u8> {
            None
        }

        fn write(&mut self, data: u8) {
            self.tx.lock().unwrap().push(data);
        }

        fn can_read(&self) -> bool {
            false
        }

        fn start_input(&mut self, cb: machina_hw_core::chardev::ByteCb) {
            let startup_byte = self.startup_byte;
            std::thread::spawn(move || {
                cb.lock().unwrap()(startup_byte);
            });
        }
    }

    let mut bus = SysBus::new("sysbus0");
    let mut address_space =
        AddressSpace::new(MemoryRegion::container("system", u64::MAX));
    let uart = Arc::new(Uart16550::new_named("uart0"));
    let tx = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(TestIrqSink::new(16));

    {
        uart.set_chardev_property("/machine/chardev/uart0").unwrap();
        uart.attach_to_bus(&mut bus).unwrap();
        let region = MemoryRegion::io(
            "uart0",
            0x100,
            Arc::new(machina_hw_char::uart::Uart16550Mmio(Arc::clone(&uart))),
        );
        uart.register_mmio(region, GPA::new(0x1000_0000)).unwrap();
        uart.attach_irq(IrqLine::new(
            Arc::clone(&sink) as Arc<dyn IrqSink>,
            10,
        ))
        .unwrap();
        uart.attach_chardev(CharFrontend::new(Box::new(LoopbackChardev {
            tx: Arc::clone(&tx),
            startup_byte: 0x51,
        })))
        .unwrap();
        let uart_for_rx = Arc::clone(&uart);
        let rx_cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
            Arc::new(Mutex::new(move |byte: u8| {
                uart_for_rx.receive(byte);
            }));
        uart.realize_onto(&mut bus, &mut address_space, rx_cb)
            .unwrap();
        uart.write(1, 0x01);
    }

    assert!(address_space.is_mapped(GPA::new(0x1000_0000), 4));

    // Poll until the RX byte arrives (start_input spawns a thread).
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let lsr = uart.read(5);
        if lsr & 0x01 != 0 {
            break;
        }
    }

    {
        assert!(uart.realized());
        assert_eq!(uart.read(0), 0x51);
        uart.write(0, 0x41);
    }

    assert_eq!(*tx.lock().unwrap(), vec![0x41]);
    assert_eq!(bus.mappings().len(), 1);
    assert_eq!(bus.mappings()[0].owner, "uart0");
}

#[test]
fn test_uart_unrealize_drops_runtime_wiring() {
    struct SinkChardev {
        tx: Arc<Mutex<Vec<u8>>>,
    }

    impl Chardev for SinkChardev {
        fn read(&mut self) -> Option<u8> {
            None
        }

        fn write(&mut self, data: u8) {
            self.tx.lock().unwrap().push(data);
        }

        fn can_read(&self) -> bool {
            false
        }
    }

    let mut bus = SysBus::new("sysbus0");
    let mut address_space =
        AddressSpace::new(MemoryRegion::container("system", u64::MAX));
    let uart = Arc::new(Uart16550::new_named("uart0"));
    let tx = Arc::new(Mutex::new(Vec::new()));

    {
        uart.attach_to_bus(&mut bus).unwrap();
        let region = MemoryRegion::io(
            "uart0",
            0x100,
            Arc::new(machina_hw_char::uart::Uart16550Mmio(Arc::clone(&uart))),
        );
        uart.register_mmio(region, GPA::new(0x1000_0000)).unwrap();
        uart.attach_chardev(CharFrontend::new(Box::new(SinkChardev {
            tx: Arc::clone(&tx),
        })))
        .unwrap();
        let rx_cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
            Arc::new(Mutex::new(move |_byte: u8| {}));
        uart.realize_onto(&mut bus, &mut address_space, rx_cb)
            .unwrap();
        uart.write(0, 0x41);
        uart.unrealize_from(&mut bus, &mut address_space).unwrap();
        uart.write(0, 0x42);
    }

    assert_eq!(*tx.lock().unwrap(), vec![0x41]);
    assert!(!address_space.is_mapped(GPA::new(0x1000_0000), 4));
    assert!(bus.mappings().is_empty());
}
