<template>
  <g id="baseLayer">
    <defs>
      <pattern id="rtgrid" width="32" height="32" patternUnits="userSpaceOnUse">
        <path d="M32 0H0V32" fill="none" stroke="#e9eff6" stroke-width="1" />
      </pattern>
      <pattern
        id="nozone"
        width="8"
        height="8"
        patternUnits="userSpaceOnUse"
        patternTransform="rotate(45)"
      >
        <rect width="8" height="8" fill="#fef2f2" />
        <line x1="0" y1="0" x2="0" y2="8" stroke="#fecaca" stroke-width="3" />
      </pattern>
      <marker id="rtarrow" markerWidth="9" markerHeight="9" refX="6" refY="4.5" orient="auto">
        <path d="M1 1 7 4.5 1 8" fill="none" stroke="#0284c7" stroke-width="1.6" />
      </marker>
    </defs>
    <rect width="800" height="460" fill="#f8fafc" />
    <rect width="800" height="460" fill="url(#rtgrid)" />
    <rect
      x="18"
      y="16"
      width="764"
      height="428"
      rx="12"
      fill="none"
      stroke="#cbd5e1"
      stroke-width="2"
      stroke-dasharray="9 7"
    />
    <rect x="42" y="38" width="150" height="96" rx="8" fill="#eaf5ee" />
    <rect x="604" y="300" width="154" height="118" rx="8" fill="#eaf5ee" />
    <path d="M20 210 H780" stroke="#e2e8f0" stroke-width="34" />
    <path d="M20 210 H780" stroke="#fff" stroke-width="2" stroke-dasharray="12 12" />
    <path d="M430 16 V444" stroke="#e2e8f0" stroke-width="28" />
    <path d="M430 16 V444" stroke="#fff" stroke-width="2" stroke-dasharray="12 12" />
    <g fill="#dbe4ef" stroke="#bccbdd" stroke-width="1.3">
      <rect x="62" y="52" width="112" height="70" rx="5" />
      <rect x="216" y="46" width="140" height="82" rx="5" />
      <rect x="486" y="50" width="128" height="76" rx="5" />
      <rect x="62" y="266" width="126" height="96" rx="5" />
      <rect x="216" y="270" width="140" height="90" rx="5" />
      <rect x="486" y="262" width="96" height="98" rx="5" />
      <rect x="636" y="322" width="98" height="66" rx="5" />
    </g>
    <g font-size="11" fill="#8296ad" font-weight="600" text-anchor="middle">
      <text x="118" y="92">A 生产车间</text>
      <text x="286" y="92">B 立体仓库</text>
      <text x="550" y="92">G 研发楼</text>
      <text x="125" y="320">D 装卸区</text>
      <text x="286" y="320">E 冷链仓</text>
      <text x="534" y="316">F 配电房</text>
      <text x="685" y="360">充电站</text>
    </g>
    <rect
      x="612"
      y="52"
      width="112"
      height="62"
      rx="6"
      fill="url(#nozone)"
      stroke="#fca5a5"
      stroke-width="1.2"
      stroke-dasharray="5 4"
    />
    <text x="668" y="88" font-size="10.5" fill="#dc2626" text-anchor="middle" font-weight="700">
      禁行区
    </text>
    <g transform="translate(88,342)">
      <rect x="-12" y="-12" width="24" height="24" rx="6" fill="#fff" stroke="#0d9488" stroke-width="1.6" />
      <path d="M1-7-6 2h5l-1 7 8-9h-5z" fill="#0d9488" />
    </g>
    <g font-size="10.5" fill="#a3b2c4" font-weight="700" letter-spacing="1">
      <text x="34" y="190">A 区</text>
      <text x="470" y="190">C 区</text>
      <text x="34" y="410">D 区</text>
      <text x="470" y="410">H 区</text>
    </g>
  </g>
</template>
