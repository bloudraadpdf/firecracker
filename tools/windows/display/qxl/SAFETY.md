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

## Display arena

The arena constructor requires one exclusively managed, live BAR range.
Allocator metadata occupies free blocks; guest and host payload access must
never reach those blocks. Allocation tokens are not copyable and retain their
mapping identity and complete reserved extent, including small tails. Release
consumes a token, or returns it unchanged for a different arena. Free-list
splitting and coalescing preserve alignment and bounds.

Raw payload pointers do not extend mapping lifetimes or prove host completion.
End all host and guest payload access before releasing its token. Stop all
users and release all tokens before reset or unmap; do not construct another
arena over memory with live tokens. Calls require external serialisation with
the display allocation semaphore. There is no implicit token deallocation.

## GDI mode buffer

Mode queries require a live miniport handle in a serialised, passive-level GDI
callback. The engine owns its zeroed allocation through destruction at
PASSIVE_LEVEL. Synchronous IOCTLs run before any reads of their output buffers;
reported byte counts and multiplication bounds are checked before iteration.
Record reads are unaligned copies of the integer-only miniport ABI.

`DrvGetModes` borrows the caller's output only after the miniport queries finish.
That output must be writable for its reported capacity and must not alias
callback-owned state. Its checked extent fits both `ULONG` and `isize` before
the Rust slice is formed. Iterators cannot outlive their owned mode buffer.

## Display output pool

The pool exclusively owns disjoint main/video arenas. Its private node headers
are separate from every payload range exposed to guest or host users. A resource
token owns one counted reference; retaining, attaching and releasing references
requires the mutable pool borrow. A final release moves all allocation tokens
off the node before freeing the storage containing it. Constructors initialise
every header field and payload byte. Tokens from another pool are returned
without node access or ownership loss.

Output tokens represent unpublished builders only. Publication consumes the
token; failure removes its temporary pending registration and returns the token.
The command ring belongs to the same device and slot generation. Payload writes
must finish before submission, and host ownership lasts until release. A safe
builder slice borrows both the token and pool, preventing submission or
cancellation while that slice remains live. Resource pointers remain raw:
callers must prove exclusive payload access before forming references.

Release-ring publication must establish host completion. Only live pending
nodes resolve an opaque release ID. Read the completed payload's packed next
ID with volatile access; validate the next live ID before freeing the current
output. Remove nodes before moving their owned references and allocation tokens.
Budget limits retain the next ID without releasing its output. Corrupt chains
leave unreleased allocations live for device-stop recovery.

`discard_pending` requires proof that the device/host has stopped all reads and
writes, including delayed completion writes. Neither a timeout nor an invalid
release ID supplies that proof. It clears only pending outputs; caller-owned
builders and cache references must also be released before mapping destruction.
`Pool::is_empty()` covers both output and resource allocations. Dropping tokens
or the pool never implicitly frees memory potentially in use by the host.

## Surface states

Only `Surface<Prepared>` exposes a safe mutable pixel slice. It owns the sole
resource reference and has not been submitted. Publication consumes that state,
so a pixel borrow cannot remain live across submission. Failure cancels the
temporary command and returns the prepared owner unchanged. `Surface<Active>`
does not expose a safe mutable slice; its raw pixel pointer needs GDI/hardware
synchronisation before access.

Both create and destroy commands retain the surface resource through their own
host completion. Successful destruction drops the active reference; a failed
publication returns that owner. The device-stop discard path requires the same
proof as pending-output discard. Allocated surface slots must describe the
correct live video mapping; encoding validates the complete pixel extent.

The PDev surface table reserves nonzero IDs below the advertised surface count.
Creation cannot reuse a live ID, and a destroy must be published before that ID
can be reused. Creation, drawing and destruction must share the same command
ordering and mapping generation. Mappings remain live through all completions.

## GDI bitmap ownership

A GDI draw area's handle owns its locked `SURFOBJ`. Drop unlocks that object
before deleting the handle. Failed creation/association/locking retains the
construction guard until it deletes any successfully allocated bitmap. GDI
access and destruction occur at PASSIVE_LEVEL in serialised display callbacks.

The secondary-surface constructor borrows the active surface through draw-area
destruction. The internal raw constructor requires a complete, live, writable
pixel mapping matching its checked layout. It must not outlive that allocation.
Neither constructor proves CPU/hardware synchronisation: callers must end
overlapping host/guest writes before GDI accesses pixel storage. Exposed surface
objects remain raw pointers and must not be retained after the owner drops.

Device-bitmap construction requires a stable, initialised driver context and
only registered hooks. The context must support deletion even during failed
construction. After handoff, GDI owns the bitmap; its deletion callback ends
the driver's context lifetime. Private helper bitmaps remain driver-owned.

PDev palettes own their `EngCreatePalette` handles through GDI device teardown.
Their raw handles in `DEVINFO` do not extend that lifetime. Native record
encoding writes only named fields, clears padding and preserves short output
buffers. Field constructors supply valid initial values, including null raw
pointers and nested arrays; native struct padding is never copied to Windows.
Thin pointer fields copy their native representation directly, preserving
provenance through the ABI record. Converting them to integer byte arrays loses
the provenance needed when a decoded pointer is used. The native record format
is for supported little-endian Windows targets, not a pointer serialisation API.
