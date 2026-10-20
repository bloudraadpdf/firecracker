# Guest driver safety boundary

The current crate builds unsigned miniport images. The display DLL, package
and native driver qualification are incomplete.

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

`ModePool` owns a stable VideoPort allocation containing an immutable mode-table
snapshot. Its internal mode-state lifetime is erased to `'static`; that state
and its borrowed slice must never be exposed or moved out. Public methods
return values or iterators tied to the owner borrow. Destruction drops state
before freeing the allocation. Construction errors retain the allocation guard
until all temporary mode-table borrows have ended. All access and destruction
require PASSIVE_LEVEL, and the device extension must remain live until then.

Uncommitted resource claims must be destroyed within HwVidFindAdapter.
`commit_discovery` is called only after every fallible setup operation succeeds,
immediately before publishing the adapter. The claim guard is the final field
to drop, so mappings and events are released before a failed claim is returned.
Configuration data is copied from its raw OS pointer; no shared Rust borrow
spans calls that can update that record.

## Callback state

The extension publishes its initialised Adapter through a release/acquire
ready flag. ISR and DPC callbacks borrow the Adapter immutably and access only
resident mappings and event handles. Paged mode storage sits in UnsafeCell and
is accessed only under VideoPort's documented request serialisation. Callbacks
must not retain their Rust references after return.

Display requests may alias their input and output buffers. Dispatch copies all
required input before constructing an output slice. The buffers must not alias
the request record or adapter state. Interface records are encoded field by
field, including explicit padding initialisation.

The packed shared RAM slot and surface fields can be only four-byte aligned.
Treat their raw u64 pointers as packed data; do not form aligned Rust u64
references. Slot registration writes individual volatile bytes and publishes
them before I/O. The shared interrupt fields are aligned u32 values.

The SYS entry wrapper delegates directly to the registered miniport code.
Panic handling calls KeBugCheckEx with the RPNC convention and source location;
it cannot unwind through the Windows boundary. Compiler memcpy/memset calls
resolve to Rust ABI wrappers. Their full ranges must be valid and meet the
VideoPort memory routines' IRQL/residency requirements. Copy ranges must not
overlap. Long ranges use ULONG-sized chunks without truncating size_t.

Framework teardown behaviour and native fault paths remain qualification work.
Linking and checking the PE images do not prove that Windows loads, unloads
or correctly dispatches the Rust driver.

## Kernel code generation

The x86 build applies LLVM's `noimplicitfloat` attribute to every function.
This prevents integer operations, copies and initialisation from introducing
SIMD or floating-point register use in interrupt handlers. It preserves the
target ABI; disabling SSE2 on Rust's standard x86 MSVC target does not.
Do not introduce explicit floating-point, vector operations or SIMD intrinsics:
the attribute does not prohibit them. Inspect the linked image's executable
sections after compiler or driver changes, including code from `core`.

Windows x64 preserves SSE state for kernel callbacks; its standard target needs
no such attribute. Neither target enables AVX. These constraints follow
[Microsoft's kernel floating-point rules](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/using-floating-point-or-mmx-in-a-wdm-driver)
and the [LLVM attribute definition](https://llvm.org/docs/LangRef.html#function-attributes).
