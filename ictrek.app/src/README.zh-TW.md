# V-Scheduler 資源調度

版本 __APP_VERSION__。

安裝前準備受保護目錄，包含 controller.json 和 database.env（SCHEDULER_DATABASE_URL），安裝時將 V_SCHEDULER_CONFIG_PATH 指向該目錄。建議使用 PGV 中專用資料庫與一般 owner 角色；不要連接平台內部資料庫或使用超級使用者。節點、執行器、預算與服務憑證由 controller 設定核准。

管理介面支援按節點檢視執行任務、等待順序與近期結束任務，提供取消、受控重試和人工刪除。連接前可檢視明確標示的演示；樣例操作不影響實際任務。介面預設跟隨 VOS 語言，可選擇簡中、英文或繁中。

每個執行節點安裝本應用後，可透過現有 projects 介面部署 resource-agent-amd 或 resource-agent-arm。AGENT_CONFIG_PATH 指向包含 agent.json 的受保護目錄，journal 設為 /journal/agent.json。支援的 NVIDIA 節點設定 AGENT_RUNTIME=nvidia；CPU 節點使用預設 runc。Agent 僅存取 controller 和已核准執行器，不直接連接資料庫。

安裝位置與計算位置分別記錄。V-Router 雲端推論不預約本地資源，本地模型以實際執行節點申請許可。未確認停止的執行保留預約；只有已停止終態任務允許軟刪除，業務輸出仍由原應用管理。

本版尚不支援自動登記/自動部署 Agent、OIDC 管理登入、HA 或多 GPU 裝置級分配。
