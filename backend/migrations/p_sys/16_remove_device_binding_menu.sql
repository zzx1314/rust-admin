-- @migration-name: remove_device_binding_menu

-- 设备绑定 was a separate page. Binding users is now done from the device list
-- page (设备列表 → 绑定用户), so the standalone menu plus its query button are
-- retired. The `/api/dogUserDevice*` and `/api/dogGateway/assign*` endpoints
-- stay in place: the device page uses them to bind/unbind users.

UPDATE p_sys_menu
SET is_deleted = 1,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id IN (1210, 1220);

DELETE FROM p_sys_role_menu
WHERE menu_id IN (1210, 1220);
