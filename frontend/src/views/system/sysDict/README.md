## 配置完字典如何使用？

答：哪里需要，根据对应字典的 dictId 查字典详情即可。示例代码如下：

```ts
import { getDictDetail } from "@/api/system";

async function getDict() {
  const { data } = await getDictDetail({ dictId: "对应字典的 dictId" });

  // data.list 即为字典详情数据
}

getDict();
```

## 温馨提示

1. 字典管理页面左侧的字典为树结构，支持字典层级分类，可将字典划分更加细致。
2. 字典管理页面左侧的字典过多后，可上下滚动，无需额外处理。
