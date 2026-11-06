const weekly = [
  { deviceCount: [1,0,2,1,0,3,1], modelCount: [0,1,1,0,2,1,0], routeCount: [2,1,0,1,1,0,2], taskCount: [3,2,4,1,2,5,3] },
  { deviceCount: [2,1,3,2,1,2,4], modelCount: [1,0,2,1,1,2,0], routeCount: [1,2,1,0,2,1,1], taskCount: [4,3,5,2,3,4,6] }
];
const delay = <T>(data: T): Promise<any> =>
  new Promise(r => setTimeout(() => r({ code: 10200, msg: "ok", data }), 100));
export const statisticsAll = () => delay([
  { name: "设备数量", value: 128, percent: "+12%", data: [1,3,2,5,4,6,8], color: "#41b6ff" },
  { name: "任务数量", value: 342, percent: "-4%", data: [4,6,5,7,6,9,7], color: "#e85f33" },
  { name: "模型数量", value: 27, percent: "+8%", data: [0,1,1,2,1,3,2], color: "#26ce83" },
  { name: "路线数量", value: 64, percent: "0%", data: [2,2,3,3,4,4,5], color: "#7846e5" }
]);
export const statisticsAllType = () => delay(weekly);
export const statisticsDocNumber = () => delay({ xdata: ["四足 A 型","四足 B 型","轮式巡检","未登记"], ydata: [62,41,20,5] });
