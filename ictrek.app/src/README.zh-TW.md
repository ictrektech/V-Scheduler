# V-Scheduler 資源調度

展開 VOS 應用群組的「V-Scheduler」，可直接進入模型與呼叫、模型與上游、推論部署、試聊、權重管理、映像倉庫、資源與任務、管理頁面及 API 文件。按「API Key」產生個人 Key 後，在結果區複製區域網路 Base URL 和 Key，模型從該 Key 可見的清單選取。無共享掛載時的權重準備規範可在「API 文件 → 跨節點模型權重傳輸」下載。

Image Studio 同一模型的主節點和每個可呼叫的運算節點各占一列。「測試」和「呼叫方式」固定到該列節點。主節點採用本機呼叫，遠端請求帶上 `compute_deployment_id`。VOS 管理員可在「模型設定」直接放行或取消放行，對應 `PUT /api/v1/model-routing`。

在「權重管理 → 模型接入狀態」分別查看權重下載、推論服務與最近測試。遠端權重由上游管理。試聊失敗會顯示所選上游、可達性及原因。

新增「推理等待佇列」與「模型倉庫 → 模型接入狀態」。試聊預設有界等待 120 秒，可查看位置並取消；應用可透過閘道排隊標頭選擇互動或背景優先順序。內建 Ollama 預設閒置保留 60 秒，同模型連續呼叫可重用；駐留期間保留完整資源預約，切換、到期或手動停止後先確認卸載再釋放。管理員可設定 0..600 秒，0 為每次卸載。服務註冊支援 metadata.model_bindings 精確關聯倉庫權重與後端模型名稱，狀態頁統一顯示路由、後端健康與排程綁定。既有模型目錄、九組連接埠、host alias 與四個容器維持相容。

統一套件包含模型閘道與調度控制器。模型呼叫使用宿主連接埠 22888 的 /v1；調度管理使用 /scheduler/api/v1。Router 直接連線應用內建的三個 Ollama 後端，不依賴 Model Hub 探索；同時自動發現 Image Studio、V-Voice、V-Music、ML Server 和影片結構化服務的可用模型。V-Voice 就緒後主動接入，自動綁定已校準的執行器與模型映射；詳見介面的「應用接入」。

V-Voice 就緒模型自動繼承應用資源預算並進入模型目錄，已設定的上游預算也適用於該上游新增的模型。可在「應用接入」和 API 文件查看接入方式及可用端點。請求及回應格式維持一致。

ARM CPU 主節點的 Image Studio SD Turbo 會在權重就緒後自動註冊已校準的執行器。在 Image Studio「生成 → 運算節點 → 主節點」可執行 F16、最大 512×512、4 步的單張生圖。V-Scheduler 任務詳細資料會顯示實際節點。生成 API 請求格式維持一致。


在側欄開啟「API 文件 → 映像檔倉庫 API」查看管理介面。請求使用 VOS 管理員 Bearer token。

已登記的運算節點對所有已驗證的 VOS 應用可見。應用依節點固定 Profile 選擇已安裝 Compose 清單中的發行映像檔並提交部署。

版本 __APP_VERSION__。

0.0.109 移除了調度控制器的容器記憶體上限，修正控制器達到舊上限時反覆重新啟動的問題。在 VOS 應用商店升級即可生效。推論資源准入、模型預算及 API 契約沿用現有規則。

安裝時會檢查 PGV 依賴。首次開啟應用並以 VOS 管理員登入後，自動建立專用資料庫、一般資料庫角色和本機資源取樣身分；設定保存在應用專屬 ${VOS_APP_STORAGE_PATH}/config 中。無需準備設定檔或執行終端命令。PGV 無法連線時頁面顯示初始化狀態。

首次設定中斷後會繼續未完成的步驟，正常重新啟動會沿用既有資料庫。已設定的執行個體若遇到資料庫或帳號異常，頁面會提示還原資料庫或修正連線設定。首次設定失敗後，依頁面提示處理並點選「重試連線」。

管理介面支援按節點檢視執行任務、等待順序與近期結束任務，提供取消、受控重試和人工刪除。連接前可檢視明確標示的演示；樣例操作不影響實際任務。介面預設跟隨 VOS 語言，可選擇簡中、英文或繁中。

安裝位置與計算位置分別記錄。V-Router 雲端推論不預約本地資源，本地模型以實際執行節點申請許可。未確認停止的執行保留預約；只有已停止終態任務允許軟刪除，業務輸出仍由原應用管理。

本版支援 VOS 管理員登入和自動本機取樣；HA 與裝置級多 GPU 分配尚未實作。

本機資源取樣已內建於應用，安裝後自動啟動，無需另行部署 Agent 或執行終端命令。支援展開逐邏輯處理器占用，Jetson Thor 的 CPU/GPU 使用同一個系統記憶體池。

管理頁提供最近 24 小時的排隊等待、執行占用 P50/P95、任務失敗率和模型許可歷史。任務詳情持續更新，支援狀態時間線、每次執行的分配/停止時間、等待原因建議及受控重試。列表可按狀態、應用、節點組合篩選，搜尋業務任務 ID；預設最近 200 條，可載入更早任務。P95 至少需要 20 個有效樣本，舊許可釋放時間未知時保持未知。

節點管理沿用已安裝 ModelInfer 的遠端節點清單與現有監控，使用者無需逐台安裝 Agent。可儲存節點預算規劃，管理已受控接入的預算、執行器和服務憑據，並檢視操作記錄。規劃不會授予執行權限；遠端調度控制介面待驗證，現有部署仍由所屬 VOS App 管理。

管理員可從 V-Scheduler 的左側導覽開啟「推論部署」，或前往「進階管理 → VOS 叢集管理」。部署清單、詳情、模型卡片和叢集檢視在目前工作區開啟。現有部署可在「節點管理 → 移轉 ModelInfer 部署」匯入。「API 文件 → 排程管理 API」提供既有移轉和執行環境介面。

在「推論部署 → 推論映像」可查閱已公開的映像位址與適用 Profile。管理員可登記映像、勾選內建 Profile 或新增自訂名稱。在「節點管理 → SSH 算力節點」編輯節點可指定這些名稱。

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

開啟駐留時，已驗證身分呼叫內建 Ollama 且未指定排隊標頭，預設有界等待 30 秒；明確指定 0、匿名呼叫及其他後端維持立即准入或拒絕。

### 應用統一接入

節點管理新增「應用統一接入」，彙總應用心跳、節點狀態、批准預算與停止請求。應用透過排程 API `/integrations` 登記已批准執行器，按回應間隔心跳；停止後必須以原執行身分確認原生退出與權重卸載才能釋放預約。節點採集隨 VOS App 自動執行，無需 SSH 或手工部署 Agent。舊應用自動綁定保持相容，尚未實作新停止協定的應用不宣稱已接管。詳見應用內「API 文件 → 應用接入」及儲存庫統一接入指南。

在使用 Traefik v3 的 VOS 上，從瀏覽器直接開啟應用根網址會跳轉到 VOS 應用頁面。嵌入頁面和 API 請求沿用原有路由。

管理員可在「資源與任務 → 執行資源」查看僅觀察節點的邏輯 CPU 數和使用率、記憶體已用／總量／可用、GPU 使用率與取樣時間。共用記憶體僅計算一次。內建推理使用已儲存的 SSH 連線取樣，缺失或過期的資料會明確標示。資料也可透過管理員排程 API GET /nodes 讀取。

### 應用後端部署

在「API 文件 → 排程管理 API → 應用後端部署」查閱權限、欄位、狀態及範例。登記節點和 Profile 後，為應用建立服務憑據，透過 PUT /backend-deployment-access/{app_id} 授權節點和倉庫。以 POST /backend-deployments 提交映像檔、目標和容器連接埠。輪詢至 running 後讀取 endpoint。SSH 節點使用 Docker，受信的 VOS 叢集成員使用 VOS Projects。停止保留主機連接埠，確認刪除後釋放。

管理員也可在「節點管理 → 應用後端」登記應用，選擇目標和 Profile。部署表單儲存映像檔、容器連接埠、選用主機連接埠、CPU 和記憶體上限及後端 JSON 設定。應用內 API 文件提供 SSH 和叢集目標的完整範例。

在「節點管理 → SSH 運算節點」新增或編輯節點時，可關閉應用後端的 Model Hub NFS，讓節點直接複製權重。停止並重新啟動現有後端後會更新掛載。先前使用共享權重的模型需重新準備。

在 Image Studio 刪除指定遠端模型時，V-Scheduler 會先確認該模型無活躍任務，再清理部署中的權重及調度登記。重試前保留刪除狀態，清理期間不接收該模型的新任務。
