const devices = Array.from({ length: 128 }, (_, i) => ({ id: i + 1, deviceId: `DOG-${i}`, status: i % 5 === 0 ? "offline" : "online" }));
const ok = () => new Promise(r => setTimeout(() => r({ code: 10200, msg: "ok", data: devices }), 100));
export const dogDeviceGetAll = ok;
export const dogDevicePage = ok;
export const dogDeviceSave = ok;
export const dogDeviceUpdate = ok;
export const dogDeviceDelete = ok;
export const dogDeviceUpdateStatus = ok;
