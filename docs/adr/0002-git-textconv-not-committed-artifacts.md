# git 集成走 textconv，不提交派生产物

`.gitattributes` 用 `diff=offdiff` 选中驱动，转换由 `diff.offdiff.textconv` 调用；仓库里只存源文件，不提交派生的 JSON 或 Markdown。

备选方案是把旁路 JSON 一起入库（托管平台的 PR 页面可见，但双写不一致、体积翻倍、评审时没人读 JSON），或依赖 external diff（两侧可见，但配置重且只在本地生效）。代价是 textconv 在 `format-patch`、`--stat` 和 GitHub/GitLab 的 PR 视图里不生效——托管评审需要改用 `offdiff export` 的产物或 CI 生成差异。

两个非显而易见的后果：同一驱动上配置 `command` 会静默顶掉 textconv，所以 external diff 必须用另一个驱动名；textconv 缓存只认命令字符串，因此命令里必须带工具版本，升级后旧缓存才会失效。
