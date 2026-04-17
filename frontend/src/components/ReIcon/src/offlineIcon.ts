// 这里存放本地图标，在 src/layout/index.vue 文件中加载，避免在首启动加载
import { getSvgInfo } from "@pureadmin/utils";
import { addIcon } from "@iconify/vue/dist/offline";

// https://icon-sets.iconify.design/ep/?keyword=ep
import EpMenu from "~icons/ep/menu?raw";
import EpEdit from "~icons/ep/edit?raw";
import EpGuide from "~icons/ep/guide?raw";
import EpSetUp from "~icons/ep/set-up?raw";
import EpMonitor from "~icons/ep/monitor?raw";
import EpLollipop from "~icons/ep/lollipop?raw";
import EpHistogram from "~icons/ep/histogram?raw";
import EpHomeFilled from "~icons/ep/home-filled?raw";
import EpNotebook from "~icons/ep/notebook?raw";
import EpBox from "~icons/ep/box?raw";
import CodiconTools from "~icons/codicon/tools?raw";
import FlowModer from "~icons/carbon/flow-modeler-reference?raw";
import SolarTuningSquare2Broken from "~icons/solar/tuning-4-outline?raw";

// https://icon-sets.iconify.design/ri/?keyword=ri
import RiMindMap from "~icons/ri/mind-map?raw";
import RiAdminFill from "~icons/ri/admin-fill?raw";
import RiTableLine from "~icons/ri/table-line?raw";
import RiLinksFill from "~icons/ri/links-fill?raw";
import RiAdminLine from "~icons/ri/admin-line?raw";
import RiListCheck from "~icons/ri/list-check?raw";
import RiSearchLine from "~icons/ri/search-line?raw";
import RiWindowLine from "~icons/ri/window-line?raw";
import RiUbuntuFill from "~icons/ri/ubuntu-fill?raw";
import RiHistoryFill from "~icons/ri/history-fill?raw";
import RiEditBoxLine from "~icons/ri/edit-box-line?raw";
import RiCodeBoxLine from "~icons/ri/code-box-line?raw";
import RiArtboardLine from "~icons/ri/artboard-line?raw";
import RiMarkdownLine from "~icons/ri/markdown-line?raw";
import RiFileInfoLine from "~icons/ri/file-info-line?raw";
import RiBankCardLine from "~icons/ri/bank-card-line?raw";
import RiFilePpt2Line from "~icons/ri/file-ppt-2-line?raw";
import RiGitBranchLine from "~icons/ri/git-branch-line?raw";
import RiSettings3Line from "~icons/ri/settings-3-line?raw";
import RiUserVoiceLine from "~icons/ri/user-voice-line?raw";
import RiBookmark2Line from "~icons/ri/bookmark-2-line?raw";
import RiFileSearchLine from "~icons/ri/file-search-line?raw";
import RiChatSearchLine from "~icons/ri/chat-search-line?raw";
import RiInformationLine from "~icons/ri/information-line?raw";
import RiTerminalWindowLine from "~icons/ri/terminal-window-line?raw";
import RiCheckboxCircleLine from "~icons/ri/checkbox-circle-line?raw";
import RiBarChartHorizontalLine from "~icons/ri/bar-chart-horizontal-line?raw";
import RiBloggerLine from "~icons/ri/blogger-line?raw";
import TdesignLockOn from "~icons/tdesign/lock-on?raw";
import RiSendPlane2Line from "~icons/ri/send-plane-2-line?raw";
import RiClapperboardAiLine from "~icons/ri/clapperboard-ai-line?raw";
import DeviconPlainGithubcopilot from "~icons/devicon-plain/githubcopilot?raw";

const icons = [
  // Element Plus Icon: https://github.com/element-plus/element-plus-icons
  ["ep/menu", EpMenu],
  ["ep/edit", EpEdit],
  ["ep/guide", EpGuide],
  ["ep/set-up", EpSetUp],
  ["ep/monitor", EpMonitor],
  ["ep/lollipop", EpLollipop],
  ["ep/histogram", EpHistogram],
  ["ep/home-filled", EpHomeFilled],
  ["ep/notebook", EpNotebook],
  ["ep/box", EpBox],
  ["codicon/tools", CodiconTools],
  ["solar/tuning", SolarTuningSquare2Broken],
  ["carbon/flow-modeler-reference", FlowModer],
  // Remix Icon: https://github.com/Remix-Design/RemixIcon
  ["ri/mind-map", RiMindMap],
  ["ri/admin-fill", RiAdminFill],
  ["ri/table-line", RiTableLine],
  ["ri/links-fill", RiLinksFill],
  ["ri/admin-line", RiAdminLine],
  ["ri/list-check", RiListCheck],
  ["ri/search-line", RiSearchLine],
  ["ri/window-line", RiWindowLine],
  ["ri/ubuntu-fill", RiUbuntuFill],
  ["ri/history-fill", RiHistoryFill],
  ["ri/edit-box-line", RiEditBoxLine],
  ["ri/code-box-line", RiCodeBoxLine],
  ["ri/artboard-line", RiArtboardLine],
  ["ri/markdown-line", RiMarkdownLine],
  ["ri/file-info-line", RiFileInfoLine],
  ["ri/bank-card-line", RiBankCardLine],
  ["ri/file-ppt-2-line", RiFilePpt2Line],
  ["ri/git-branch-line", RiGitBranchLine],
  ["ri/settings-3-line", RiSettings3Line],
  ["ri/user-voice-line", RiUserVoiceLine],
  ["ri/bookmark-2-line", RiBookmark2Line],
  ["ri/file-search-line", RiFileSearchLine],
  ["ri/chat-search-line", RiChatSearchLine],
  ["ri/information-line", RiInformationLine],
  ["ri/terminal-window-line", RiTerminalWindowLine],
  ["ri/checkbox-circle-line", RiCheckboxCircleLine],
  ["ri/bar-chart-horizontal-line", RiBarChartHorizontalLine],
  ["ri/blogger-line", RiBloggerLine],
  ["tdesign/lock-on", TdesignLockOn],
  ["ri/send-plane-2-line", RiSendPlane2Line],
  ["ri/clapperboard-ai-line", RiClapperboardAiLine],
  ["devicon/githubcopilot", DeviconPlainGithubcopilot]
];

// 本地菜单图标，后端在路由的 icon 中返回对应的图标字符串并且前端在此处使用 addIcon 添加即可渲染菜单图标
icons.forEach(([name, icon]) => {
  addIcon(name as string, getSvgInfo(icon as string));
});
