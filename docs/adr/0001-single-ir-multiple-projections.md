# 单一 Document IR，多投影

OffDiff 只维护一份权威结构化表示（Document IR），JSON 产物、Markdown 投影与 Diff 视图都从它渲染，Markdown 永远不是 git 的比对口径。

备选方案是让 JSON 与 Markdown 各自独立解析源文件，或让 git 直接 diff Markdown。前者会让两套语义漂移、每个特性实现两遍；后者会把 Markdown 表达不了的格式变更从 git diff 里抹掉，与本项目的目标直接冲突。
