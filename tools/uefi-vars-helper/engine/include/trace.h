// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-or-later
#pragma once
/* QEMU trace events are diagnostic; do not expose variable values in logs. */
#define trace_uefi_variable(action, name, size, guid) do { (void)(action); (void)(name); (void)(size); (void)(guid); } while (0)
#define trace_uefi_status(action, status) do { (void)(action); (void)(status); } while (0)
#define trace_uefi_vars_policy_deny(reason) ((void)(reason))
#define trace_uefi_vars_policy_cmd(name) ((void)(name))
#define trace_uefi_vars_security_violation(reason) ((void)(reason))
#define trace_uefi_vars_proto_cmd(name) ((void)(name))
#define trace_uefi_event(name) ((void)(name))
