//! WS63 LED Blinky example.
//!
//! Blinks the onboard LED on a WS63 evaluation board.
//! The LED is typically connected to GPIO0 on most WS63 EVBs.
//!
//! This example demonstrates:
//! - Runtime initialization (ws63-rt)
//! - GPIO output control (ws63-hal)
//! - Simple busy-wait delay

#![no_std]
#![no_main]

use ws63_hal::gpio::create_output_pin;
use ws63_rt::entry;

/// Simple busy-wait delay (~240 MHz CPU clock).
fn delay_ms(ms: u32) {
    // Approximate: 240 cycles = 1 µs at 240 MHz
    for _ in 0..ms {
        for _ in 0..240_000 {
            core::hint::spin_loop();
        }
    }
}

#[entry]
fn main() -> ! {
    // Configure GPIO pin 0 as output (board LED on WS63 EVB)
    let mut led = create_output_pin(0);

    loop {
        // LED on
        led.set_high();
        delay_ms(500);

        // LED off
        led.set_low();
        delay_ms(500);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
