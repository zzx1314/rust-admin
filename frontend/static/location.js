/* 设备定位地图页：拉取 /api/location，绘制最新点 + 轨迹 + 精度圈。 */
(function () {
  "use strict";

  var FALLBACK_CENTER = [34.7466, 113.6253]; // 郑州中心，仅当无任何坐标时居中
  var map = L.map("map", { zoomControl: true }).setView(FALLBACK_CENTER, 5);
  L.tileLayer("https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png", {
    maxZoom: 19,
    attribution: '&copy; OpenStreetMap',
  }).addTo(map);

  var marker = null;       // 最新点
  var accuracyCircle = null;
  var trajLine = null;     // 轨迹折线

  function fmt(v, d) { return (v === null || v === undefined || v === "") ? "—" : v; }

  function setText(id, v) {
    var el = document.getElementById(id);
    if (el) el.textContent = fmt(v);
  }

  function methodBadge(method) {
    if (method === "cell") return '<span class="badge b-ok">基站精确定位</span>';
    if (method === "ip") return '<span class="badge b-warn">IP 城市级</span>';
    return '<span class="badge b-none">未解析</span>';
  }

  function renderLatest(latest) {
    if (!latest) {
      setText("method", "无数据");
      setText("ts", "尚未采集");
      return;
    }
    document.getElementById("method").innerHTML = methodBadge(latest.method);
    // 精度：基站定位才显示理论半径；IP 定位仅模糊标注为城市级（运营商骨干网出口），不展示具体米数
    if (latest.method === "cell") {
      setText("accuracy", latest.accuracy ? latest.accuracy + " m · 理论精度" : "—");
    } else if (latest.method === "ip") {
      setText("accuracy", "城市级（运营商骨干网出口）");
    } else {
      setText("accuracy", "—");
    }
    setText("coords", (latest.lat != null && latest.lng != null)
      ? latest.lat.toFixed(6) + ", " + latest.lng.toFixed(6) : "—");
    setText("ts", latest.ts || (latest.ts_epoch ? new Date(latest.ts_epoch * 1000).toLocaleString() : "—"));

    setText("operator", latest.operator || "—");
    setText("mcc", latest.mcc);
    setText("mnc", latest.mnc);
    setText("tac", latest.tac + (latest.tac_hex ? " (0x" + latest.tac_hex + ")" : ""));
    setText("ci", latest.ci + (latest.ci_hex ? " (0x" + latest.ci_hex + ")" : ""));
    setText("rat", latest.rat);
    setText("rsrp", latest.rsrp_dbm != null ? latest.rsrp_dbm + " dBm" : "—");

    var note = document.getElementById("note");
    if (latest.method === "ip") {
      note.style.display = "block";
      note.innerHTML = '<div class="hint">⚠️ <b>IP 城市级定位</b>：指向<b>运营商骨干网出口</b>，' +
        '并非设备精确位置，误差约数公里（不展示具体精度数值）。若需精确到小区（几十~几百米），' +
        '请确保已联网以查询 cellocation / OpenCellID 基站库。</div>';
    } else if (latest.method === "cell") {
      note.style.display = "block";
      note.innerHTML = '<div class="hint">✅ <b>基站精确定位</b>（cellocation 国内库 / OpenCellID），' +
        '坐标贴附于实际小区铁塔，accuracy 为基站库返回的理论精度半径。' +
        (latest.address ? ' 地址：' + latest.address : '') + '</div>';
    } else {
      note.style.display = "block";
      note.innerHTML = '<div class="hint">⚠️ 暂未解析出坐标：' +
        (latest.error ? latest.error : "设备可能未联网(PDP 未激活)，或小区未入库。已记录小区号，联网后自动解析。") +
        '</div>';
    }
  }

  function renderMap(latest, trajectory) {
    // 轨迹折线：仅用有坐标的点
    var pts = (trajectory || []).filter(function (r) {
      return r.lat != null && r.lng != null;
    }).map(function (r) { return [r.lat, r.lng]; });

    if (trajLine) { map.removeLayer(trajLine); trajLine = null; }
    if (pts.length >= 2) {
      trajLine = L.polyline(pts, { color: "#2f6df0", weight: 3, opacity: 0.7 }).addTo(map);
    }

    if (latest && latest.lat != null && latest.lng != null) {
      var ll = [latest.lat, latest.lng];
      if (!marker) {
        marker = L.marker(ll).addTo(map);
      } else {
        marker.setLatLng(ll);
      }
      // 画笔：基站定位用精度半径实线圈；IP 定位用固定大圈(仅视觉提示，不标具体米数)并以暖色虚线区分
      var acc = latest.method === "cell" ? (latest.accuracy || 0) : 8000;
      if (accuracyCircle) { map.removeLayer(accuracyCircle); }
      if (acc > 0) {
        var isIP = latest.method === "ip";
        var opts = { radius: acc, color: isIP ? "#e0a030" : "#2f6df0",
                     fillOpacity: 0.1, weight: 1 };
        if (isIP) opts.dashArray = "4,4";
        accuracyCircle = L.circle(ll, opts).addTo(map);
      }
      marker.bindPopup(
        "<b>设备最新位置</b><br>" +
        "方式：" + (latest.method === "cell" ? "基站精确定位" : latest.method === "ip" ? "IP 城市级" : "未解析") + "<br>" +
        "坐标：" + latest.lat.toFixed(6) + ", " + latest.lng.toFixed(6) + "<br>" +
        "精度：" + (latest.method === "cell"
          ? (latest.accuracy ? latest.accuracy + " m · 理论精度" : "—")
          : "城市级（运营商骨干网出口）") + "<br>" +
        "时间：" + (latest.ts || "") + "<br>" +
        "TAC/CI：0x" + (latest.tac_hex || "?") + " / 0x" + (latest.ci_hex || "?") +
        (latest.address ? "<br>地址：" + latest.address : "")
      ).openPopup();

      if (pts.length >= 1) {
        map.fitBounds(L.latLngBounds(pts), { padding: [40, 40], maxZoom: 16 });
      } else {
        map.setView(ll, 15);
      }
    } else if (pts.length >= 1) {
      map.fitBounds(L.latLngBounds(pts), { padding: [40, 40], maxZoom: 16 });
    }
  }

  function fetchAndRender() {
    fetch("/api/location?limit=300", { cache: "no-store" })
      .then(function (r) { return r.json(); })
      .then(function (data) {
        renderLatest(data.latest);
        renderMap(data.latest, data.trajectory);
        document.getElementById("last-fetch").textContent =
          "最近更新：" + new Date().toLocaleTimeString();
      })
      .catch(function (e) {
        document.getElementById("last-fetch").textContent = "拉取失败：" + e;
      });
  }

  document.getElementById("btn-refresh").addEventListener("click", function () {
    this.disabled = true;
    this.textContent = "采集中…";
    fetch("/api/location/refresh", { method: "POST" })
      .then(function () { return fetchAndRender(); })
      .finally(function () {
        this.disabled = false;
        this.textContent = "立即刷新定位";
      }.bind(this));
  });

  fetchAndRender();
  setInterval(fetchAndRender, 30000); // 每 30s 自动刷新
})();
