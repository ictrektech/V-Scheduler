# V-Scheduler 資源調度

新增「推理等待佇列」與「模型倉庫 → 模型接入狀態」。試聊預設有界等待 120 秒，可查看位置並取消；應用可透過閘道排隊標頭選擇互動或背景優先順序。內建 Ollama 預設閒置保留 60 秒，同模型連續呼叫可重用；駐留期間保留完整資源預約，切換、到期或手動停止後先確認卸載再釋放。管理員可設定 0..600 秒，0 為每次卸載。服務註冊支援 metadata.model_bindings 精確關聯倉庫權重與後端模型名稱，狀態頁統一顯示路由、後端健康與排程綁定。既有模型目錄、九組連接埠、host alias 與四個容器維持相容。

統一套件包含模型閘道與調度控制器。模型呼叫使用宿主連接埠 22888 的 /v1；調度管理使用 /scheduler/api/v1。Router 直接連線應用內建的三個 Ollama 後端，不依賴 Model Hub 探索；同時自動發現 Image Studio、V-Voice、V-Music、ML Server 和影片結構化服務的可用模型。V-Voice 就緒後主動接入，自動綁定已校準的執行器與模型映射；詳見介面的「應用接入」。

版本 __APP_VERSION__。

安裝時會檢查 PGV 依賴。首次開啟應用並以 VOS 管理員登入後，自動建立專用資料庫、一般資料庫角色和本機資源取樣身分；設定保存在應用專屬 ${VOS_APP_STORAGE_PATH}/config 中。無需準備設定檔或執行終端命令。PGV 無法連線時頁面顯示初始化狀態。

管理介面支援按節點檢視執行任務、等待順序與近期結束任務，提供取消、受控重試和人工刪除。連接前可檢視明確標示的演示；樣例操作不影響實際任務。介面預設跟隨 VOS 語言，可選擇簡中、英文或繁中。

安裝位置與計算位置分別記錄。V-Router 雲端推論不預約本地資源，本地模型以實際執行節點申請許可。未確認停止的執行保留預約；只有已停止終態任務允許軟刪除，業務輸出仍由原應用管理。

本版支援 VOS 管理員登入和自動本機取樣；HA 與裝置級多 GPU 分配尚未實作。

本機資源取樣已內建於應用，安裝後自動啟動，無需另行部署 Agent 或執行終端命令。支援展開逐邏輯處理器占用，Jetson Thor 的 CPU/GPU 使用同一個系統記憶體池。

管理頁提供最近 24 小時的排隊等待、執行占用 P50/P95、任務失敗率和模型許可歷史。任務詳情持續更新，支援狀態時間線、每次執行的分配/停止時間、等待原因建議及受控重試。列表可按狀態、應用、節點組合篩選，搜尋業務任務 ID；預設最近 200 條，可載入更早任務。P95 至少需要 20 個有效樣本，舊許可釋放時間未知時保持未知。

節點管理沿用已安裝 ModelInfer 的遠端節點清單與現有監控，使用者無需逐台安裝 Agent。可儲存節點預算規劃，管理已受控接入的預算、執行器和服務憑據，並檢視操作記錄。規劃不會授予執行權限；遠端調度控制介面待驗證，現有部署仍由所屬 VOS App 管理。

## Host alias、內部連接埠與宿主機連接埠

以下為所有 profile 共用的預設映射。`<vos-host>` 為安裝此應用的 VOS 宿主機 IP 或網域名稱；192、229、232 三台測試機分別使用 `192.168.1.192`、`192.168.1.229`、`192.168.1.232`。內部位址僅在 Docker `vos_default` 網路解析，完整 URL 在位址前加 `http://`。安裝時修改連接埠參數後，以實際安裝值為準；應用內「API 文件 → 模型管理 API」顯示設定後的位址。

統一容器的正式 host alias 只有 `v-scheduler`，透過 `8080`、`38120`、`6005` 區分閘道、排程和模型管理；此容器其餘名稱全部保留為相容別名。新應用與既有應用後續適配均建議使用 `v-scheduler`。三個 Ollama 獨立容器保留各自名稱。

| 功能 | Docker host alias:內部連接埠（含全部相容別名） | 宿主機 host:外部連接埠 | 主要路徑與用途 |
|---|---|---|---|
| 統一推理、閘道管理及統一介面 | `v-scheduler:8080`; 相容：`v-scheduler-gateway:8080`、`v-router:8080`、`v-router-backend:8080` | `<vos-host>:22888` | `/v1/*` 推理；`/api/v1/*` 閘道管理；`/` 介面 |
| 調度接管與資源管理 | `v-scheduler:38120`; 相容：`v-scheduler-backend:38120`、`v-scheduler-frontend:38120` | `<vos-host>:38120` | `/api/v1/*` 任務、許可、節點、執行器及統計 |
| 模型倉庫管理 | `v-scheduler:6005`; 相容：`v-scheduler-model-hub:6005`、`v-scheduler-model-hub-backend:6005` | `<vos-host>:36005` | `/api/v1/*` 模型、選取檔案下載、更新、匯入、別名、引用與後端管理；`/openapi.json` |
| Ollama QA 原生管理 | `v-scheduler-ollama-qa:11434` | `<vos-host>:12434` | `/api/*` 模型清單、拉取、詳情、載入/卸載及原生推理 |
| Ollama QA 推理閘道 | `v-scheduler-ollama-qa:11535` | `<vos-host>:12535` | `/v1/chat/completions` 聊天；`/metrics` 指標 |
| Ollama Embedding 原生管理 | `v-scheduler-ollama-embedding:11434` | `<vos-host>:12435` | `/api/*` 原生模型管理及向量推理 |
| Ollama Embedding 推理閘道 | `v-scheduler-ollama-embedding:11535` | `<vos-host>:12536` | `/v1/embeddings` 向量；`/metrics` 指標 |
| Ollama Rerank 原生管理 | `v-scheduler-ollama-rerank:11434` | `<vos-host>:12436` | `/api/*` 原生模型管理及重排推理 |
| Ollama Rerank 推理閘道 | `v-scheduler-ollama-rerank:11535` | `<vos-host>:12537` | `/v1/rerank` 重排；`/metrics` 指標 |

可設定的外部連接埠參數：

- `V_ROUTER_HOST_PORT`：預設 `22888`。
- `SCHEDULER_HOST_PORT`：預設 `38120`。
- `MODEL_HUB_BACKEND_HOST_PORT`：預設 `36005`。
- `OLLAMA_QA_HOST_PORT`：預設 `12434`。
- `OLLAMA_QA_GATEWAY_HOST_PORT`：預設 `12535`。
- `OLLAMA_EMBEDDING_HOST_PORT`：預設 `12435`。
- `OLLAMA_EMBEDDING_GATEWAY_HOST_PORT`：預設 `12536`。
- `OLLAMA_RERANK_HOST_PORT`：預設 `12436`。
- `OLLAMA_RERANK_GATEWAY_HOST_PORT`：預設 `12537`。

同一列的相容別名指向相同容器與連接埠，不產生額外服務或宿主機映射。`host.docker.internal` 是閘道容器中的 Docker 宿主機別名（`host-gateway`），用於存取宿主機已發布的連接埠，例如 `host.docker.internal:38120`；它不是新增服務，也沒有獨立的固定連接埠。

閘道亦在相同 `22888` 連接埠提供 `/scheduler/api/v1/*` 調度代理及 `/model-hub/api/v1/*` 模型管理代理。VOS 內的統一入口為 `/app/com.ictrek.v-scheduler/`；模型管理前綴為 `/app/com.ictrek.v-scheduler/model-hub/api/v1/*`，閘道 API 前綴為 `/api/com.ictrek.v-scheduler`。三個 Ollama 的 `11535` 是各後端推理/指標入口；跨後端統一模型名稱及調度入口使用 `v-scheduler:8080/v1/*` 或 `<vos-host>:22888/v1/*`。

這些別名由 V-Scheduler 自行宣告；原 Model Hub 的別名、連接埠及共用目錄維持原應用所有權。共用模型目錄仍於安裝時選取，沿用原 Model Hub 選取的公共目錄即可。
