// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-or-later
#include "qemu/osdep.h"
#include "hw/uefi/var-service.h"
#include "hw/uefi/var-service-api.h"
#include "var-service-vars.c"
#include "var-service-policy.c"

typedef struct {
    uefi_vars_state state;
} Engine;
typedef struct {
    uint8_t guid[16];
    const uint8_t *name, *data, *digest;
    uint32_t name_size, data_size, digest_size, attributes;
    uint8_t time[16];
} Variable;

int qemu_uuid_is_equal(const QemuUUID *a, const QemuUUID *b)
{
    return memcmp(a, b, sizeof(*a)) == 0;
}
QemuUUID qemu_uuid_bswap(QemuUUID uuid)
{
    uuid.fields.time_low = __builtin_bswap32(uuid.fields.time_low);
    uuid.fields.time_mid = __builtin_bswap16(uuid.fields.time_mid);
    uuid.fields.time_high_and_version = __builtin_bswap16(uuid.fields.time_high_and_version);
    return uuid;
}
char *qemu_uuid_unparse_strdup(const QemuUUID *uuid)
{
    return g_strdup_printf("%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
                           uuid->data[0], uuid->data[1], uuid->data[2], uuid->data[3],
                           uuid->data[4], uuid->data[5], uuid->data[6], uuid->data[7],
                           uuid->data[8], uuid->data[9], uuid->data[10], uuid->data[11],
                           uuid->data[12], uuid->data[13], uuid->data[14], uuid->data[15]);
}
void uefi_vars_json_save(uefi_vars_state *state)
{
    (void)state;
}

Engine *fcuv_new(void)
{
    Engine *e = g_new0(Engine, 1);
    QTAILQ_INIT(&e->state.variables);
    QTAILQ_INIT(&e->state.var_policies);
    e->state.max_storage = 1024 * 1024;
    e->state.force_secure_boot = true;
    e->state.disable_custom_mode = true;
    e->state.buffer = g_malloc0(MAX_BUFFER_SIZE);
    e->state.buf_size = MAX_BUFFER_SIZE;
    return e;
}
void fcuv_free(Engine *e)
{
    uefi_vars_clear_all(&e->state);
    uefi_vars_policies_clear(&e->state);
    g_free(e->state.buffer);
    g_free(e);
}
void fcuv_cold_boot(Engine *e)
{
    uefi_vars_clear_volatile(&e->state);
    uefi_vars_policies_clear(&e->state);
    e->state.end_of_dxe = e->state.ready_to_boot = e->state.exit_boot_service =
        e->state.policy_locked = false;
    uefi_vars_auth_init(&e->state);
}
uint32_t fcuv_flags(Engine *e)
{
    return e->state.end_of_dxe | (e->state.ready_to_boot << 1) | (e->state.exit_boot_service << 2) |
           (e->state.policy_locked << 3);
}
void fcuv_set_flags(Engine *e, uint32_t flags)
{
    e->state.end_of_dxe = flags & 1;
    e->state.ready_to_boot = flags & 2;
    e->state.exit_boot_service = flags & 4;
    e->state.policy_locked = flags & 8;
}
bool fcuv_variable(Engine *e, uint32_t index, Variable *out)
{
    uefi_variable *v;
    QTAILQ_FOREACH(v, &e->state.variables, next)
    {
        if (index-- != 0)
            continue;
        memcpy(out->guid, &v->guid, 16);
        out->name = (void *)v->name;
        out->name_size = v->name_size;
        out->data = v->data;
        out->data_size = v->data_size;
        out->digest = v->digest;
        out->digest_size = v->digest_size;
        out->attributes = v->attributes;
        memcpy(out->time, &v->time, 16);
        return true;
    }
    return false;
}
bool fcuv_put_variable(Engine *e, const Variable *in)
{
    uefi_variable v = {0};
    memcpy(&v.guid, in->guid, 16);
    memcpy(&v.time, in->time, 16);
    v.name = (void *)in->name;
    v.name_size = in->name_size;
    v.data = (void *)in->data;
    v.data_size = in->data_size;
    v.digest = (void *)in->digest;
    v.digest_size = in->digest_size;
    v.attributes = in->attributes;
    if (v.name_size > 65536 || v.data_size == 0 || v.data_size > e->state.max_storage ||
        uefi_vars_post_load(&v, 1) != 0 ||
        uefi_vars_find_variable(&e->state, v.guid, v.name, v.name_size))
        return false;
    uefi_vars_set_variable(&e->state, v.guid, v.name, v.name_size, v.attributes, v.data,
                           v.data_size);
    uefi_variable *stored = uefi_vars_find_variable(&e->state, v.guid, v.name, v.name_size);
    stored->time = v.time;
    stored->digest_size = v.digest_size;
    stored->digest = g_memdup2(v.digest, v.digest_size);
    uefi_vars_update_storage(&e->state);
    return e->state.used_storage <= e->state.max_storage;
}
const uint8_t *fcuv_policy(Engine *e, uint32_t index, uint32_t *size)
{
    uefi_var_policy *p;
    QTAILQ_FOREACH(p, &e->state.var_policies, next)
    {
        if (index-- != 0)
            continue;
        *size = p->entry_size;
        return (void *)p->entry;
    }
    return NULL;
}
bool fcuv_put_policy(Engine *e, const uint8_t *data, uint32_t size)
{
    if (size < sizeof(variable_policy_entry) || size > 65536)
        return false;
    uefi_var_policy p = {.entry = (void *)data, .entry_size = size};
    if (p.entry->version != VARIABLE_POLICY_ENTRY_REVISION ||
        p.entry->offset_to_name < sizeof(variable_policy_entry) ||
        p.entry->lock_policy_type > VARIABLE_POLICY_TYPE_LOCK_ON_VAR_STATE ||
        p.entry->min_size > p.entry->max_size)
        return false;
    if (p.entry->lock_policy_type == VARIABLE_POLICY_TYPE_LOCK_ON_VAR_STATE) {
        uint32_t start = sizeof(variable_policy_entry) + sizeof(variable_lock_on_var_state);
        if (p.entry->offset_to_name < start || p.entry->offset_to_name > size ||
            !uefi_str_is_valid((void *)(data + start), p.entry->offset_to_name - start, true))
            return false;
    }
    if (uefi_var_policy_post_load(&p, 1) != 0)
        return false;
    uefi_vars_add_policy(&e->state, p.entry);
    uefi_vars_update_storage(&e->state);
    return e->state.used_storage <= e->state.max_storage;
}
uint32_t fcuv_transact(Engine *e, uint8_t *buffer, uint32_t size)
{
    if (size < sizeof(mm_header) || size > MAX_BUFFER_SIZE)
        return UEFI_VARS_STS_ERR_BAD_BUFFER_SIZE;
    mm_header header;
    memcpy(&header, buffer, sizeof(header));
    if (header.length != size - sizeof(header))
        return UEFI_VARS_STS_ERR_BAD_BUFFER_SIZE;
    memset(e->state.buffer, 0, MAX_BUFFER_SIZE);
    memcpy(e->state.buffer, buffer, size);
    uint32_t status = UEFI_VARS_STS_SUCCESS;
    if (qemu_uuid_is_equal(&header.guid, &EfiSmmVariableProtocolGuid))
        status = uefi_vars_mm_vars_proto(&e->state);
    else if (qemu_uuid_is_equal(&header.guid, &VarCheckPolicyLibMmiHandlerGuid))
        status = uefi_vars_mm_check_policy_proto(&e->state);
    else if (qemu_uuid_is_equal(&header.guid, &EfiEndOfDxeEventGroupGuid))
        e->state.end_of_dxe = true;
    else if (qemu_uuid_is_equal(&header.guid, &EfiEventReadyToBootGuid))
        e->state.ready_to_boot = true;
    else if (qemu_uuid_is_equal(&header.guid, &EfiEventExitBootServicesGuid))
        e->state.exit_boot_service = true;
    else
        status = UEFI_VARS_STS_ERR_NOT_SUPPORTED;
    uefi_vars_update_storage(&e->state);
    uint32_t variables = 0, policies = 0;
    uefi_variable *v;
    uefi_var_policy *p;
    QTAILQ_FOREACH(v, &e->state.variables, next) variables++;
    QTAILQ_FOREACH(p, &e->state.var_policies, next) policies++;
    if (variables > 4096 || policies > 4096 || e->state.used_storage > e->state.max_storage)
        return UINT32_MAX;
    memcpy(buffer, e->state.buffer, size);
    return status;
}
