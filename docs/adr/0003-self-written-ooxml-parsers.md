# OOXML 自研解析器，PDF 用纯 Rust 库

docx / xlsx / pptx 用 zip + roxmltree 自研解析，不引入 docx-rs、calamine 或 umya-spreadsheet。

原因：三个格式都需要解析样式继承与主题才能得到有效属性，而现成库要么完全不提供样式（calamine），要么要求调用方自己补继承链（docx-rs），自研才能同时满足有效属性、确定性输出与 diff 局部性。附加约束：本机 cargo 目前只能离线构建，未进入缓存的依赖拉不进来。

代价是解析器维护成本由我们承担。PDF 没有语义结构，自研成本过高，改用纯 Rust 的 unpdf（需先进入本机依赖缓存）。
