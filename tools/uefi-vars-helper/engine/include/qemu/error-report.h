// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-or-later
#pragma once
#define error_report(...) do { fprintf(stderr, __VA_ARGS__); fputc('\n', stderr); } while (0)
#define warn_report(...) error_report(__VA_ARGS__)
