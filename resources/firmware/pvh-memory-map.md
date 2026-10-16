# Preserve the PVH memory map

Apply `cloudhv-pvh-memory-map.patch` after the other Firecracker patches to
Cloud Hypervisor EDK II revision `97eeb7b0901da9f7b5d98a2e6a5e8694978298e9`. It
is required by the headless, graphical and Secure Boot CloudHv profiles.

The pinned firmware discarded non-RAM PVH entries and published one flat RAM
resource below 4 GiB. Windows could consequently allocate page tables inside
Firecracker's reserved 32–33 MiB platform region. Updating the VM clock's
disruption marker on snapshot restore then corrupted an OS page table.

The patch publishes each usable interval separately, rounds RAM inward and
reserved ranges outward to pages, and preserves the page-zero allocation
reservation. It validates bounded entry counts, arithmetic and overlapping
ranges before size discovery or HOB publication. Invalid maps stop firmware;
they cannot fall back to a flat CMOS-derived map. Non-CloudHv RAM discovery is
unchanged. EDK II's GCD converts reserved resource HOBs into reserved UEFI
memory descriptors; no competing system-memory HOB covers these ranges.

Existing snapshots retain their old guest memory map. Cold boot with corrected
firmware before creating replacement snapshots. Do not patch a running guest's
page tables or suppress VM clock updates.
