# OPS-OBS 观察期操作手册（A2b）

日期：2026-10-26（Asia/Shanghai）  
基线 tip：`28c0328f8fa6a0ea9c577674b58787fb013e4f62`（A2a OBS-MARK）。采集机若停在其后仅文档合入的 tip，把 `git rev-parse HEAD` 的完整 commit 写入现场表。  
API 真名以 `crates/agent-value-store/src/observation.rs` 为准。

本文是操作清单。合入本文不开始观察采集，不改阈值，不发版。模型 id（Q1）、kapibala 主机（Q2）、负臂 key-env 名（Q3）尚未锁定。未锁定项保持 `TBD`。现场表里仍有 `TBD` 时，禁止调用 `register_window`。`TBD` 与空白不是建议默认值，不得把举例抄进去充当已锁定事实。

## 1. 目标

操作者按本清单：

1. 书面预注册连续 2 个 Asia/Shanghai 自然日。
2. 确认被测官方指纹基线已存在且仍有效，然后才登记窗口。
3. 窗内只用手工「跑一次」。每臂每日至少 1 次完整成功写入（N=1）。
4. 窗结束后调用 `audit_window`，再查 `audit_ready`。

同日若密钥、月预算、探针同意、老板书面同意、正臂协议或基线有任一项不齐：发当日 BLOCKED，不开窗。

## 2. 非目标

- 不改 `DETERMINISTIC_PASS_MIN`、`DETERMINISTIC_FAIL_MAX`、`FINGERPRINT_PASS_MIN`、`FINGERPRINT_FAIL_MAX`、τ_JS 的算法、`OBSERVATION_RUN_DAYS`。操作者只核对冻结值等于 tip：确定性通过下界 9、不通过上界 7；指纹通过下界 8、不通过上界 5；观察窗天数 2。τ_JS 不是全局常数，随被冻结的基线。
- 不打 tag，不发 Release，不切 CHANGELOG 版本节，不 bump 版本。C1′ v0.3.0 已经发出。
- A3（桌面一键采基线）不与本观察采集同日进行。
- Frequency / 频次仍为 Blocked。本刀不读、不改频次。
- 不改 SKILL、探针判分、store schema、常量、桌面、web、tools。
- 不把密钥值写入仓库、日志、PR 或群消息。
- 不用「更便宜」或「更慢」代替负臂指纹 Fail。见第 5 节原句。

## 3. 正臂与负臂

### 3.1 正对照：官方直连

- `ProviderKind` 必须是 `Official`。
- 模型是被测官方模型。完整 model id 由老板另锁（Q1）。本文不预填任何 model id。
- 路线主机是该厂商官方 API 主机，并且与臂上 `Route::host`、探针 base 的主机一致。具体主机在开跑前写入现场表，不在本文写成默认值。
- 协议优先走现 tip 已支持的 `openai-chat-*` 路径，协议名为 `openai-chat-completions`。Anthropic Messages 未核实。未核实则当日 BLOCKED，原因写「正臂协议缺口」，不硬跑。

### 3.2 负对照：较小模型

按顺序选型，不得跳步：

1. 优先：同一厂商族的较小官方模型，仍走 `ProviderKind::Official`。较小指同族更小档。具体 model id 属于 Q1，本文不锁定。任何档位举例都不是默认被测，也不是默认冒充模型。
2. 否则：经 kapibala 中转的较小模型，`ProviderKind::Relay`。中转主机须老板书面确认（Q2）。确认前现场表保持 `TBD`。夹具或笔记里出现过的主机名不是生产默认，不得抄进现场表。
3. 负臂 model id 必须在开跑前书面预注册，且与正臂不同。两臂相同时 `register_window` 返回 Conflict：`positive and negative arms must differ`。
4. 负臂与正臂共用同一被测基线（窗冻结的 `baseline_key` 与 `baseline_content_sha256`）。负臂日谓词只看指纹 Fail。

## 4. 预注册现场表

登记时刻必须严格早于 `window_start` 当日 00:00（Asia/Shanghai）。Q1、Q2、Q3 未锁定时不得调用 `register_window`。

| 字段 | 现场值 | 说明 |
|---|---|---|
| `window_start`（北京日期 D） |  | 操作者选定。注册必须在 D 日 00:00 之前 |
| 正臂 model id | TBD | Q1。老板书面锁定后才可替换 |
| 正臂 `ProviderKind` | Official | 官方直连 |
| 正臂 `Route::host` |  | 与官方 API 主机、探针 base 主机一致 |
| 正臂协议 | 优先 `openai-chat-completions` | Anthropic Messages 未核实 → BLOCKED「正臂协议缺口」 |
| 正臂 key-env 名 |  | 只写名：`OPENAI_API_KEY` 或 `ANTHROPIC_API_KEY`。不写值 |
| 负臂 model id | TBD | Q1。须与正臂不同 |
| 负臂选型 | 先同厂商较小 Official，否则 kapibala Relay | 顺序已定。id 与主机未定 |
| 负臂 `ProviderKind` | TBD | 随选型填 `Official` 或 `Relay` |
| kapibala / Relay host | TBD | Q2。老板书面确认前保持 TBD |
| 负臂 key-env 名 | TBD | Q3。优先填本次 `--key-env` 或臂绑定名。候选名只有 `RELAY_API_KEY`、`KAPIBALA_API_KEY`、`KAPIBALA_GPT_API_KEY`。若调用方只读 `RELAY_API_KEY`，在本行声明映射。不写值 |
| `baseline_key` |  | 须已在 `fingerprint_baselines` 且未过期 |
| 三类 probe id / version | 见第 7.3 节 | 必须与 tip `probe-run` 一致 |
| 四阈值与窗天数 | 由 `register_window` 冻结 | 操作者只核对，不手填注入 |
| `git commit` |  | 采集机 `git rev-parse HEAD` |
| `PROBE_SET_VERSION` | `1` | tip 现值 |
| 操作者 |  |  |
| 登记时刻（Asia/Shanghai） |  | 必须早于 D 日 00:00 |

`register_window` 自动冻结、调用方不可注入：`baseline_content_sha256`、`tau_js`、`det_pass_min=9`、`det_fail_max=7`、`fp_pass_min=8`、`fp_fail_max=5`、`observation_run_days=2`、`end_date=D+1`、`end_exclusive` 为 (D+2) 日北京 00:00 的 UTC 毫秒。登记后核对这些值等于 tip 常量。

另抄进现场记录、并在窗内保持同一套：`git_commit`、锁定后的负模型 ID、操作者、登记时刻（北京）。

## 5. 负对照预期差异方向（B0 §5）

下列句子原文照录，不改字：

> 负对照要打的是指纹相对被测官方基线应 Fail（较小模型冒充被测时 token 分布应偏离）；成本/时延/质量分数不必更差，不得用「更便宜/更慢」代替指纹 Fail。

## 6. 日谓词（N=1）

日集是闭区间 `[start, end_date]`，恰 2 个连续 Asia/Shanghai 自然日。运行日按该次探针完成写入的时刻换算。

N=1：每一臂、每一个北京日，至少 1 次完整成功写入，并进入当日合并。禁止事后挑成功日。禁止用累计 `pass_days≥2` 凑满。

合并：`DaySummary::build` 里 Fail 优于 Pass，再优于 Uncertain，再优于 N/A。任一条 hard failure 使该日为 hard。指纹行的 baseline key 或 content 与窗冻结值不一致，该日失败。

| 臂 | 窗内每一日 |
|---|---|
| 正 | `¬hard`，且 `det=Pass ∧ cap∈{Pass,N/A} ∧ fp=Pass`，并且确定性、能力、指纹三类都有行 |
| 负 | `¬hard`，且只有指纹须为 Fail：`fp=Fail`。该指纹行的 key 与 content 必须等于冻结双字段。不要求确定性或能力通过 |

`Uncertain` 不是 Fail。负臂若是 Uncertain、Pass、缺指纹行，或出现 hard，该日都不算通过。正臂的 Uncertain 也不是 Pass。

`window_ok` 当且仅当两个北京日的正臂与负臂都通过。失败窗完整保留。下一窗使用新的 `start`。同一 `start` 不能再次 `register_window`。

本样本通过后，文案上限是「观察期校验通过（DECISIONS 13.8）」或「观察期参考校准已合入」。不得写「已统计校准」「已验证最优阈值」「观察期科学完成」。本手册不宣布观察期完成。校验失败时先查工程、题集、基线、负对照选型，不先改阈值。

## 7. A2a API

只用下列方法。不要发明其它名字。`audit_ready` 只读最终快照，不调用 legacy `authenticity::observation()`。

### 7.1 控制臂标记

| API | 作用 |
|---|---|
| `Store::set_control(ControlRole, &Arm)` | 把当前对照臂标为 positive 或 negative。该 role 已经是 Set 时，先 `clear_control` |
| `Store::clear_control(ControlRole)` | 清除该 role |
| `Store::list_observation_controls()` | 历史，旧的在前 |

`ControlRole` 为 `Positive` 或 `Negative`。`ControlAction` 为 `Set` 或 `Clear`。窗口在 `register_window` 时直接冻结正负 `Arm`。controls 是可查询的当前对照臂标记。开跑前冻结臂与 controls 必须一致。

### 7.2 预注册窗

| API | 作用 |
|---|---|
| `Store::register_window(&RegisterObservationWindow)` | 同一事务写入 intent 与 window。冻结基线双字段、四常量、τ_JS 文本、三类探针引用 |
| `Store::void_window(window_id)` | 仅在 start 北京午夜之前，且两臂日集都还没有探针结果时可以作废。同一 start 不能再登记 |
| `Store::list_observation_windows()` | 列出窗口。`status` 为 `Open`、`Ended` 或 `Void` |

调用方传入 `RegisterObservationWindow` 的字段：`positive`、`negative`、`start`、`baseline_key`，以及确定性、指纹、能力三类的 probe id 与 version。

基线是硬前置。键不存在时 Conflict 为 `unknown fingerprint baseline`。基线已过期、不是 active 时 Conflict 为 `fingerprint baseline is not active`。没有已存在且仍有效的基线，不得调用 `register_window`。

时间：

- 开放：未 void，且 `now < end_exclusive`。
- 已结束：`now >= end_exclusive`。
- 注册必须在 start 的北京 00:00 之前。过晚则 Conflict：`observation window must be registered before its Beijing start`。
- 同一 start 已登记则 Conflict：`observation window already registered for this start`。

### 7.3 探针 id

`register_window` 的三类 id 与 version 必须与下表一致，否则日谓词对不上，窗口失败。

| class | probe_id | version |
|---|---|---|
| Deterministic | `openai-chat-deterministic` | `1` |
| Capability | `openai-chat-capability` | `1` |
| Fingerprint | `openai-chat-fingerprint` | `1` |

version 等于 `PROBE_SET_VERSION`（`"1"`）。

### 7.4 审计

| API | 作用 |
|---|---|
| `Store::audit_window(window_id)` | 窗结束后写入最终快照。窗口仍开放时 Conflict：`observation window is still open`，不写快照。已有快照则原样返回 |
| `Store::audit_ready()` | 存在未 void 的最终快照且 `window_ok=1` 时为真 |
| `Store::audit_report(window_id)` | 只读已存最终快照。没有快照则 Conflict：`observation window has no final snapshot` |

## 8. 操作顺序

1. `git rev-parse HEAD`。确认是 `28c0328f8fa6a0ea9c577674b58787fb013e4f62`，或其后仅文档合入的 tip。把完整 commit 写入现场表。
2. 确认被测官方模型的指纹基线已经采集（`collect_fingerprint_baseline` 或等价 upsert），键可查，且未超过 30 天。记下 `baseline_key`。没有基线就停。
3. 填写第 4 节现场表。任一格仍是 `TBD` 就停。
4. 走第 9 节。任一项未勾，改走第 10 节，不要登记窗口。
5. 该 role 已经 Set 时先 `clear_control`。然后 `set_control(Positive, &pos)` 与 `set_control(Negative, &neg)`。
6. `register_window(...)`。`start` 是明日或更后的北京日期，且当前时刻早于该日 00:00。
7. `list_observation_windows`：状态为 Open；冻结阈值等于 tip 常量；正负臂与 controls 一致。
8. 每个北京日对正臂、负臂各至少手工跑一次完整探针（桌面「跑一次」，或单次 `probe-run` / `probe_live.py`）。正臂写入三类。负臂至少写入指纹类。定时保持关闭。
9. `now >= end_exclusive` 之后：`audit_window(id)`，看 `window_ok`，再 `audit_ready()`。要读已存快照时用 `audit_report(id)`。
10. 窗口失败：完整保留。用新的 `start` 重新登记。不要对同一 start 再调 `register_window`。

作废只用于还没到 start 午夜、且两臂日集都没有探针的窗口：`void_window(window_id)`。作废之后，同一 start 也不能再登记。

## 9. 开跑清单

全部勾选之后才可以 `register_window`。

- [ ] tip 已核（`28c0328f8fa6a0ea9c577674b58787fb013e4f62`，或其后 docs tip）
- [ ] 探针同意已接受（桌面 consent / `probe_consent`）
- [ ] 月预算未触顶（`PROBE_MONTHLY_BUDGET_YUAN` 为 10，含基线采集估价）
- [ ] 老板已书面同意开跑对照，群内可追溯
- [ ] 正臂协议是现 tip 探针支持的 `openai-chat-*` 路径。Anthropic Messages 未核实则本项不勾，改为 BLOCKED「正臂协议缺口」
- [ ] 正臂密钥环境变量名在本机可用：`OPENAI_API_KEY` 或 `ANTHROPIC_API_KEY`。只确认名字。值留在 gitignored `.env` 或进程环境，不写入本清单
- [ ] 负臂密钥环境变量名已按 Q3 锁定，且本机可用。名字为 `RELAY_API_KEY`、`KAPIBALA_API_KEY`、`KAPIBALA_GPT_API_KEY` 之一；若调用方只读 `RELAY_API_KEY`，现场表已声明映射。不写值
- [ ] 正负 base URL 的主机与臂 `Route::host` 一致
- [ ] 被测官方指纹基线已存在且未过期
- [ ] 正负 model id 已书面锁定，现场表不再是 TBD，且负臂不等于正臂
- [ ] 若负臂走 Relay：kapibala 主机已由老板书面确认，现场表不再是 TBD
- [ ] 三类 probe id / version 等于第 7.3 节
- [ ] `window_start` 仍在未来（注册先于 D 日 00:00 北京时间）
- [ ] Frequency 仍不动
- [ ] 应用内定时关闭。本窗只用手工「跑一次」

## 10. 当日 BLOCKED

第 9 节缺任一项，当日在群内发送下面模板。只写环境变量名，并注明「有名无值」或「无名」。不粘贴密钥值。不粘贴把 host、path、key 拼在一起的串。

缺什么勾什么。当日必须发出。

```
【A2b OPS-OBS BLOCKED】YYYY-MM-DD Asia/Shanghai
缺：
- [ ] 密钥：正臂 env 名 ___ / 负臂 env 名 ___（有名无值或无名）
- [ ] 预算：本月已用估价将触顶或无法确认
- [ ] 同意：探针 consent 未接受 / 老板未书面同意开跑
- [ ] 协议：正臂协议缺口（Anthropic Messages 未核实，或不是 tip 的 openai-chat-* 路径）
- [ ] 基线：被测官方指纹基线不存在或已过期
- [ ] 预注册：model id / kapibala host / key-env 仍为 TBD（Q1–Q3）
48h 截止：YYYY-MM-DD HH:MM Asia/Shanghai
逾期未补 → 观察采集改 Not started 挂起（C1′ v0.3.0 已发，不跳 Release）
```

截止写完整的 `YYYY-MM-DD HH:MM Asia/Shanghai`。48 小时从这条 BLOCKED 消息算起。

## 11. 48 小时止损

C1′ v0.3.0 已在 tip 发出。满 48 小时后，观察采集改为 Not started 挂起。不要改走「跳过并发布 C1′」。

| 时点 | 动作 |
|---|---|
| T0 当日 | 群内 BLOCKED，列出缺项 |
| T0 起 48 小时仍缺 | 观察采集状态改为 Not started（挂起）。停止等待 |
| 明确不做 | 不跳 C1′。不改阈值。不 tag。不无限等 |
| 文档 | 本 runbook 可以继续合入。开跑挂起不等于文档停摆 |

挂起后若要重启：新的老板书面同意、补齐全部缺项，并预注册一个新窗口（新的 `start`）。旧窗保留。

## 12. 手工跑一次

观察窗内默认只做手工一次：桌面「跑一次」，或等价的单次 `probe-run` / `probe_live.py`。应用内定时默认关闭。

`PROBE_SCHEDULE_INTERVAL_MS` 为 24 小时，是暂定调度间隔，不是观察期阈值。本刀不改这个常量。若有人另行打开定时，只在现场笔记记下该调度参数。不要把它写成观测阈值，也不要用它代替 N=1 的按日手工覆盖。

## 13. 密钥

本文件与现场沟通只允许出现下列环境变量名：

- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `RELAY_API_KEY`
- `KAPIBALA_API_KEY`
- `KAPIBALA_GPT_API_KEY`

仓库 `.env.example` 只列前三个名字。真实值只放在本地 gitignored `.env` 或进程环境。agent-value 只保存变量名。不要把值写入 git、PR、CI 日志、群消息或本文件。中转调用时，值放在调用方声明的 `--key-env` 所指向的变量里。
