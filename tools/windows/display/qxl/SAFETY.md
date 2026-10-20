# Guest driver safety boundary

The current crate contains driver components, not loadable drivers. The Windows
entry points, ownership of mapped regions and installation are still pending.

## Windows ABI

`videoport.rs` uses Microsoft's documented VideoPort interfaces. Windows
`BOOLEAN` is `u8`, `BOOL` is `i32`, `ULONG` is `u32`, and `ULONG_PTR` is `usize`.
Calls and callbacks use `extern "system"`: stdcall on x86 and the Windows x64
ABI on x64. The physical address argument uses the 64-bit integer representation
of `PHYSICAL_ADDRESS`. Unused callback slots remain raw pointers and must be
null when the miniport does not supply those callbacks.

Compile-time assertions check the structure sizes and key offsets separately
for x86 and x64. The vendor reference drivers also initialise 84-byte and
144-byte initialisation records respectively; the x86 callbacks pop their
arguments. These checks support the declarations but do not prove native
execution of the Rust replacement.

All VideoPort calls are unsafe. Callers must provide live, correctly sized
buffers, satisfy the function's documented IRQL, and retain allocations and
callbacks for as long as Windows can use them. The request packet can alias its
input and output buffers; consume input before writing output. Do not create
overlapping Rust references for those buffers.

These are kernel interfaces. Do not substitute user-mode GDI imports, a CRT,
VC-LTL or YY-Thunks for kernel imports. Raw DLL imports remove SDK import
libraries from the build; they do not remove the Windows ABI contract.

## Shared rings

`ring.rs` requires a live mapping, consistent cache attributes and one
serialised guest endpoint for each ring. The command and cursor endpoints own
their producer fields; the release endpoint owns its consumer fields. The host
owns the other endpoint. Ring headers use four-byte-aligned volatile access;
packed payloads use byte access. Fences preserve publication and notification
ordering.

The constructors check mapping length, alignment and capacity. Operations check
counter distance. A full or empty result arms a notification before rechecking
the counters. The caller must perform the indicated host notification. Stop all
endpoint access before reset or unmap, and create new endpoints after reset.

## Addresses and modes

`slots.rs` computes integer addresses only. It does not dereference them or
extend a mapping's lifetime. Its results still require a live mapping before
they can become pointers. A decoded buffer must match both slot and generation
and fit wholly inside the slot.

`modes.rs` parses an immutable byte snapshot. A caller must not form an ordinary
Rust slice over device memory that changes during the borrow. Mode requests
validate the aligned framebuffer size before they change state.

## Miniport resources

`DeviceResources::discover` requires a live VideoPort device extension and
PASSIVE_LEVEL. The extension must outlive every mapping and event. Stop all
interrupts, DPCs and display-driver access before resource destruction, which
also requires PASSIVE_LEVEL. Resource ownership does not itself stop callbacks.

Each successful mapping or event allocation has one owner. Early returns drop
the owners already constructed, including a successful but short mapping.
Memory access stays within the requested BAR length even if Windows returns
a larger mapping. `Mapping::address` returns a raw pointer; it does not permit
access after unmap. I/O port pointers use wrapping arithmetic because they are
opaque port addresses, not Rust memory allocations.
