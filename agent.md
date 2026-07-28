# retana 设计文档

## retana设计目标

retana是一个跨平台的本地hermes前端系统，它为远程hermes实例提供

- 一个类似IM聊天的交互UI，并能显示hermes当前的具体操作
- 一个可以远程执行命令行的SSH通道

## retana架构

retana应当由多个部分组成

- 一个反向ssh执行的完成
- 一个本地的定时hook服务，用于完成本地任务
- 一套关于本地机器环境的简要记忆文件
- 一个用于唤起聊天窗口的服务，及聊天窗口的具体实现

retana应当采用tauri实现，并在

- MacOS 26+
- Windows 11
- Linux Plasma

中提供正常工作的所有必要组建
