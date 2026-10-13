# V-Scheduler 资源调度

版本 __APP_VERSION__。

配置固定存放在应用专属 ${VOS_APP_STORAGE_PATH}/config 中，无需选择公共配置目录。安装前在该目录准备 controller.json 和 database.env（SCHEDULER_DATABASE_URL），目录权限 0700、文件权限 0600。推荐使用 PGV 中专用数据库和普通 owner 角色；不要连接平台内部库或使用超级用户。节点、执行器、预算和服务凭据由 controller 配置批准。

管理界面支持按节点查看执行任务、等待顺序和近期结束任务，提供取消、受控重试和人工删除。连接前可查看明确标注的演示；样例操作不影响真实任务。界面默认跟随 VOS 语言，可选择简中、英文或繁中。

每个执行节点安装本应用后，可通过现有 projects 接口部署 resource-agent-amd 或 resource-agent-arm。AGENT_CONFIG_PATH 指向含 agent.json 的受保护目录，journal 设置为 /journal/agent.json。支持的 NVIDIA 节点设置 AGENT_RUNTIME=nvidia；CPU 节点使用默认 runc。Agent 只访问 controller 和已批准执行器，不直接连接数据库。

安装位置与计算位置分别记录。V-Router 云端推理不预约本地资源，本地模型以实际执行节点申请许可。未确认停止的执行保留预约；只有已停止终态任务允许软删除，业务输出仍由原应用管理。

本版尚不支持自动登记/自动部署 Agent、OIDC 管理登录、HA 或多 GPU 设备级分配。
