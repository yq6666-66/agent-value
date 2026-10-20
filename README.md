# agent-value

English | [简体中文](README.zh-CN.md)

agent-value is a local desktop tool for one person. It scores the (model, route) combinations you actually use on cost, time and quality, and weights the three by your own preferences. It is not a public leaderboard: every number comes from your own tasks, and all data stays on your machine.

The scoring rules follow a written specification (v22) plus a decision record for the gaps the specification leaves open. Both are kept outside this repository; this README describes the parts that are implemented.

## Status

There is no usable app yet. The formulas below exist as a tested Rust library, and the local database exists as a separate crate, but they are not wired together and there is no interface beyond a placeholder page. Nothing is scored end to end.

| Spec step | What it covers | Status |
|---|---|---|
| 1 | Constants, scoring scopes, arms, time windows, weight versions | Implemented |
| 2 | Local SQLite data model and privacy rules | Implemented (not yet connected to the desktop shell) |
| 3 | Task outcome rules and quality signals | Implemented |
| 4-5 | Cell states and cell weights $\omega_u$; prior, posterior and retry model | Implemented |
| 6-7 | Cell cost, time and quality $C^{\ast}, T^{\ast}, Q^{\ast}$; aggregation; value score $V$ | Implemented |
| 8 | Eligibility gates $\mathcal E_d$ | Implemented (authenticity and log integrity are passed in by the caller until step 13) |
| 9 | Frontier and diagnostic table | Implemented (per layer and overall; the display is planned) |
| 10 | Reference anchor tiers I / II / III, reference and fallback arm selection, official picks | Implemented (no display layer yet) |
| 11 | Weight elicitation from your answers and behaviour | Implemented (library only; no question screen or suggestion display yet) |
| 12 | Weight sensitivity display | Planned |
| 13 | Probes and relay checks | Planned |

Changes by version are in [CHANGELOG.md](CHANGELOG.md).

## Key ideas

- **Scoring scope.** Each task type is scored separately. By default there are two scopes: coding and non-coding. Scores are never combined across scopes.
- **Arm.** The unit being compared: a model plus a route. A route is the API host, the protocol (for example Anthropic Messages or OpenAI Chat Completions) and the agent or client. If any of the three differs, it is a different arm. Changing the API key on the same route does not make a new arm.
- **Cell and layer.** Tasks are grouped into cells $u$ by subtype (for coding by default: fix, add, refactor, explain and review, other). By default there is one layer; optionally, tasks can be split into layers $d$ by size (small, medium, large), measured only on your own input, and a cell is then a subtype within a layer. Each cell has a weight $\omega_u$ equal to its share of the layer's tasks in the previous window.
- **Task.** One stretch from your submission to delivery, abandonment or cancellation.
- **Time window.** Scoring runs in 30-day windows. Cell weights, your weight version, price tables and classification rules for a window are fixed from earlier data when the window starts; the reference and fallback arms are frozen the same way once chosen (the rules for choosing them are in the next item). In the first window $V$ is withheld; the library already does this. By the specification the first window still shows each arm's cost, time and quality and the frontier, labelled as not yet calibrated. The library computes the frontier, but showing it with that label is planned.
- **Reference arm $R_k$ and fallback arm $F_k$.** $R_k$ is the arm every other arm is compared against. $F_k$ is the arm a task is handed to after an arm fails $K$ times. The reference and fallback arms do not themselves hand off to anyone. $R_k$ is the first available arm in this order: the arm you chose, the scope's official direct arm, and the arm with the most tasks in the previous window. An arm is available if in the previous window it passed the authenticity gate and had no $F_{\rm bad}$ cell; equal task counts go to the arm whose id sorts first. If several official direct arms exist they are tried in order of previous-window tasks; when the one with the most tasks is unavailable, the next official direct arm is used, and the most-tasks arm overall is used only when no official direct arm is available. By default $F_k=R_k$. You can choose a different $F_k$. It is kept only if in the previous window it passed the authenticity gate, had no $F_{\rm bad}$ cell, was tier I or tier II (section 5) in every positive-weight layer, and had at least 3 ratings ($n_q\ge3$) in every positive-weight cell. Otherwise, including when it has no positive-weight layer at all, $F_k$ reverts to $R_k$ and the version record notes this with the reasons.

## How scoring works

Notation: $i$ is an arm, $u$ a cell, $d$ a layer. Subscripts are dropped where the meaning is clear; unless stated otherwise every quantity is per arm, per cell. $\mathrm{NA}$ means "not defined", and the code never replaces it with 0.

### 1. Task outcomes and quality signals

*Spec §3.1. Implemented.*

Each task gets exactly one outcome. The rules are checked from top to bottom and the first one that applies wins:

1. You explicitly abort the task: **cancelled**.
2. The task ran checks (tests, build, type check) and the last one failed: **failed**.
3. Within 30 minutes after the task ends, you send a message in the same session or working directory that the frozen classifier marks as a redo or correction: **failed**. Asking for a new feature does not count as a redo.
4. For coding tasks, the task's changes are fully reverted within 24 hours: the outcome is **changed to failed** after the fact.
5. The agent reports completion, the last check (if any) passed, and rules 3 and 4 did not fire: **success**.
6. Anything else (incomplete logs, no completion signal, interrupted session): **undetermined**. Undetermined is never counted as success.

Results stay provisional until the 30-minute redo window closes, and for coding tasks also the 24-hour rollback window. These outcomes give the per-cell counts $n^1$ (successes), $n^0$ (failures), $n^C$ (cancellations) and $m$ (undetermined).

Quality $q_j\in[0,1]$ is measured only on successful tasks and only from your ratings: a 1-5 star rating maps to $q_j=(s-1)/4$. To avoid rating only the tasks that went wrong, the core library picks a random 10% of successful tasks for a rating request, at most 3 per day (Beijing time); the prompt that asks you is planned. How much of a change was kept is never used as quality, since it would reward small edits and punish reasonable refactors; it is meant only as a diagnostic.

Each layer uses one of two quality modes per window:

- **Rating mode**, when every arm in the candidate list the caller passes in (meant to be the eligible set $\mathcal E_d$ from section 6; the library does not connect the two yet) has at least 3 ratings in every positive-weight cell. The reference or fallback arm counts only if it is itself a candidate. Ratings are shrunk toward a pooled mean $q_B$ (the previous window's mean over all arms in that cell, or $0.5$ if there is none):

$$
\hat q=\mathrm{clip}\!\left(\frac{\sum_{j=1}^{n_q}q_j+m_0q_B}{n_q+m_0},\ 0.01,\ 0.99\right),\qquad m_0=0.5 .
$$

- **Success-rate mode**, otherwise. Every arm in the layer, including the reference arm, gets $\hat q=0.99$, so $Q^{\ast}$ becomes the probability of delivering successfully. The results are to be labelled that way (planned with the display), rather than presenting missing data as average quality.

### 2. Cell weights and cell states

*Spec §0. Implemented.*

**Cell weights.** For window $t$, $\omega_u$ is the number of your tasks in cell $u$ during window $t-1$ divided by your tasks in the whole layer, counted by tasks, not by cost or time, and excluding probe tasks. If the layer had no tasks, the previous valid weights are carried over; if there are none, the configured subtypes get equal weight. Only cells with $\omega_u>0$ take part in any count or score below. The design reserves a separate probe cell whose weight is always 0 (probes themselves are planned, step 13).

**Cell states.** Every positive-weight cell of every arm is in exactly one state:

$$
\mathrm{miss}:\quad n^1+n^0=0,
$$

$$
\mathrm{fail}:\quad n^1+n^0>0\ \wedge\ \bigl(n^1+n^0<3\ \vee\ p_K^{lo}<0.75\ \vee\ p_K^{C,lo}<0.75\bigr),
$$

and $\mathrm{valid}$ otherwise. A failing cell is one of two kinds, checked in this order:

- $F_{\rm thin}$ (too few samples): $n^1+n^0<3$. The pessimistic pass rate is not checked, because it is unreliable with so few samples.
- $F_{\rm bad}$ (truly failing): $n^1+n^0\ge3$ and $p_K^{lo}<0.75$ or $p_K^{C,lo}<0.75$.

$p_K^{lo}$ and $p_K^{C,lo}$ are defined in the next section. For each layer the code also counts the cells in each state and sums their weights; the reference arm uses exactly the same rules. In a $\mathrm{miss}$ cell, $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$ and their cancel-robust versions are all $\mathrm{NA}$.

### 3. Pass rate and retries

*Spec §1. Implemented.*

For each arm in each cell, these quantities give the chance that the arm finishes a task within its own $K$ attempts, and how many failed attempts to expect along the way.

**Prior.** Each cell starts from a base pass rate $p_B$: a direct benchmark if one exists, otherwise the mean of the same layer, otherwise $0.5$. The prior strength $\kappa$ sets how much weight $p_B$ gets against your own outcomes:

$$
\kappa=
\begin{cases}
n_B, & \text{direct pooling } (\rho=1),\\[2pt]
\dfrac{\kappa_p\,n_B}{\kappa_p+n_B}, & \text{discounted } (0\le\rho<1),\\[6pt]
\min(\kappa_p,3), & \text{same-layer mean used for } p_B,\\
0, & \text{no benchmark and no layer mean (Jeffreys)},
\end{cases}
\qquad \kappa_p=20\rho .
$$

$n_B$ is the benchmark's sample size and $\rho$ is how closely the benchmark matches your own tasks. $\kappa$ jumps at $\rho=1$; this follows the specification and is listed as an open question.

**Posterior first-try pass rate.**

$$
\alpha=\kappa p_B+n^1+0.5,\qquad
\beta=\kappa(1-p_B)+n^0+0.5,\qquad
p_1=\frac{\alpha}{\alpha+\beta}.
$$

The $+0.5$ terms are Jeffreys half-counts, so with no prior at all ($\kappa=0$) the arm starts from $\mathrm{Beta}(0.5,0.5)$. Cancelled and undetermined tasks do not enter $\alpha$ or $\beta$.

**Retries.** An arm gets at most $K=3$ attempts of its own. After a failure, each further attempt succeeds with probability $\pi$. For now $\pi$ is passed in by the caller; estimating it from your data is planned. If all $K$ attempts fail, the task is handed to the fallback arm $F_k$, which may try up to $K$ more times, so the worst case is $2K=6$ attempts per task.

$$
q_K=(1-p_1)(1-\pi)^{K-1},\qquad p_K=1-q_K,
$$

$$
E[n_F]=(1-p_1)\sum_{j=0}^{K-1}(1-\pi)^{j},\qquad
E[n_W]=E[n_F]-q_K .
$$

| Symbol | Meaning |
|---|---|
| $p_K$ | Probability that the arm succeeds within its own $K$ attempts |
| $q_K$ | Probability that all $K$ of the arm's own attempts fail and the task is handed off |
| $E[n_F]$ | Expected number of the arm's own failed attempts |
| $E[n_W]$ | Expected number of waits before a retry. Every failure among the first $K-1$ attempts is followed by one more try, whatever that try's result. The $K$-th failure goes straight to the fallback arm with no wait, so it is not counted. |

All four describe only the arm's own attempts; retries inside the fallback arm are not included. The fallback's cost, time and quality enter once per task through the $q_K$ terms in section 4.

**Pessimistic pass rate and cancel-robust variant.** The cell-state test uses $p_1^{5\%}=\mathrm{Beta}^{-1}(0.05;\alpha,\beta)$ and $p_K^{lo}=1-(1-p_1^{5\%})(1-\pi)^{K-1}$. The cancel-robust variant counts cancellations as failures, $\tilde n^0=n^0+n^C$, and recomputes $p_1^C$, $p_K^C$, $q_K^C$, $E^C[n_F]$, $E^C[n_W]$ and $p_K^{C,lo}$ with the same formulas.

<details>
<summary>Implementation notes</summary>

- $q_K$ is computed first and $p_K$ is set to $1-q_K$, so $p_K+q_K=1$ exactly. $E[n_F]$ is summed term by term rather than with the closed form $(1-p_1)\frac{1-(1-\pi)^K}{\pi}$; this avoids cancellation error at very small $\pi$ and needs no special case at $\pi=0$, where it reduces to $K(1-p_1)$.
- The fail test does not invert the Beta distribution. It computes $x^{\ast}=1-0.25/(1-\pi)^{K-1}$; if $x^{\ast}\le0$ the cell passes, otherwise it fails when $\mathrm{BetaCDF}(x^{\ast};\alpha,\beta)>0.05$. This is equivalent to $p_K^{lo}<0.75$. The quantile is found by bisection on the CDF and is used only for display. The incomplete Beta function is implemented in this crate.
- The prior is chosen by an explicit enum (`DirectPool`, `Discounted(ρ)` with $\rho\in[0,1)$, `LayerMean`, `Jeffreys`), not by comparing $\rho$ to 1. `Discounted` gives $\kappa=0$ when $\kappa_p+n_B=0$; `Discounted(1.0)` and $\rho<0$ return an error.

</details>

### 4. Cell cost, time and quality: $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$

*Spec §2-§3. Implemented.*

These are the expected cost, active time and delivered quality of giving one task in a cell to an arm, counting retries, cancellations and handoffs.

**Inputs.** The cost $c$ of a task is the actual charge for all its requests when available (relay balance change or bill lines, de-duplicated by call ID), otherwise token usage (input, output, cache read, cache write, including any relay multiplier) times the frozen price table, otherwise $\mathrm{NA}$. The library takes costs that are already in RMB; the following rules are the input convention for whoever assembles them (the assembly is planned). Subscription routes split the monthly fee by the task's share of that month's tokens; free credits are valued at list price, not 0; classifier and grading-model costs are excluded. The time $t$ is active time: wall-clock time minus periods spent waiting for you (permission prompts, questions to you). Waiting time and the number of interruptions are meant to be shown next to $T^{\ast}$ as diagnostics (planned with the display).

**Per-side means.** For each side (successful tasks, failed tasks), let $m_S$ or $m_F$ be the number of tasks on that side whose value is not $\mathrm{NA}$, so $m_S\le n^1$ and $m_F\le n^0$. Writing $m^{\prime}$ for either count, the mean is winsorized: with $m^{\prime}\ge10$, the $k(m^{\prime})=\max(1,\lfloor0.01m^{\prime}\rfloor)$ largest and $k$ smallest values are replaced by the nearest remaining value; with $m^{\prime}<10$ the raw mean is used and marked as not winsorized. The mean is then shrunk toward the benchmark:

$$
\tilde C_S=\frac{m_S\bar C_S^w+\kappa_C C_S^B}{m_S+\kappa_C},\qquad
\tilde C_F=\frac{m_F\bar C_F^w+\kappa_C C_F^B}{m_F+\kappa_C},\qquad
\kappa_C=5 .
$$

Time uses the same formulas. Without a benchmark there is no shrinkage and $\tilde C=\bar C^w$. If a side has no samples, its value falls back to the direct benchmark, then to the arm's pooled mean over the layer, then to $\mathrm{NA}$.

**Cost.**

$$
C^*_{iu}=p_{K}\tilde C_{S}+E[n_F]\tilde C_{F}
+q_{K}\,C^*_{F,u}
+\frac{n^C\hat{\bar C}_{C}}{n^1+n^0} .
$$

Read left to right: the cost of the successful attempt, the cost of the failed attempts, the cost of handing off to the fallback arm, and the cost of cancelled tasks spread over the completed ones. $\hat{\bar C}_C$ is the mean cost of a cancelled task; without cancellation bills it falls back to $\max(\tilde C_F,\ \text{layer pool})$, then $\max(\tilde C_F,\ C_F^B)$, then $\mathrm{NA}$. For the reference and fallback arms themselves the handoff term is 0, because they are the last stop.

**Time.**

$$
T^*_{iu}=p_{K}\tilde T_{S}+E[n_F]\tilde T_{F}
+E[n_W]D+q_{K}T^*_{F,u}
+\frac{n^C\bar T_{C}}{n^1+n^0} .
$$

$D$ is the interval between a failure and the next retry. If it is missing, the layer's pooled value is used, then the benchmark; with neither, time is not comparable and no total score is produced.

**Quality.**

$$
Q^*_{iu}=p_{K}\hat q+q_{K}\bar Q_{F},\qquad
\bar Q_{F}=\frac{f\,Q^{\rm match}+\kappa_F Q^*_{F,u}}{f+\kappa_F},\qquad \kappa_F=5 .
$$

$\bar Q_F$ is the quality you get when the fallback arm finishes a task this arm failed: the observed quality on the $f$ matched cases, shrunk toward the fallback arm's own cell quality. For the reference arm and the fallback arm themselves, the handoff term is 0: $Q^{\ast}_{R,u}=p_K\hat q$.

**Missing data stays missing.** If the reference arm's value in a cell is $\mathrm{NA}$ for any reason, every arm's value in that cell is $\mathrm{NA}$ for that dimension. When $F_k\ne R_k$ and the fallback arm has $\mathrm{NA}$ in any dimension, all three of the candidate's dimensions in that cell are $\mathrm{NA}$. There is no second "bare" cost that leaves out the handoff term.

**Cancel-robust versions.** These replace $p_K$, $q_K$, $E[n_F]$ with the cancel-robust values and replace the cancellation term with the extra cost of cancelled tasks beyond a normal failure:

$$
C^{*,C}_{iu}=p^C_{K}\tilde C_S+E^C[n_F]\tilde C_F+q^C_K C^*_{F,u}
+\frac{E_{C}}{n^1+n^0},
$$

$$
E_{C}=
\begin{cases}
\displaystyle\sum_{j\in\mathcal C}\max\bigl(0,\ c_j-\tilde C_{F}\bigr), & \text{per-task bills},\\[8pt]
E^{UB}_{C}, & \text{aggregate bill with a per-task cap},\\[4pt]
E^{LB}_{C}, & \text{aggregate bill with no cap, or no cancel bills},
\end{cases}
$$

$$
E^{LB}_{C}=n^C\max\bigl(0,\bar C_{C}-\tilde C_{F}\bigr),
$$

$$
E^{UB}_{C}
=\left\lfloor\frac{S_{C}}{U}\right\rfloor\max\bigl(0,\ U-\tilde C_{F}\bigr)
+\max\!\left(0,\ S_{C}-\left\lfloor\frac{S_{C}}{U}\right\rfloor U-\tilde C_{F}\right),
\qquad S_{C}=n^C\bar C_{C}.
$$

$U$ is the per-task cap and $S_C$ the total billed for cancelled tasks. $E^{UB}$ is the largest possible extra cost given the total and the cap; $E^{LB}$ is a lower bound. When only the lower bound is available, the cell is flagged and the robust comparison is turned off for it. Time uses the same formulas. Finally a monotone guard makes the robust versions never look better than the plain ones:

$$
C^{*,C}\leftarrow\max(C^{*,C},C^*),\qquad
T^{*,C}\leftarrow\max(T^{*,C},T^*),\qquad
Q^{*,C}\leftarrow\min(Q^{*,C},Q^*),
$$

with $Q^{\ast,C}_{iu}=p^C_K\hat q+q^C_K\bar Q_F$.

<details>
<summary>Implementation notes</summary>

- The specification writes the first term of $E^{UB}$ as $\lfloor S_C/U\rfloor(U-\tilde C_F)$, which goes negative when $U<\tilde C_F$. The code takes $\max(0,\cdot)$ on that term. The two agree whenever $U\ge\tilde C_F$. The expression is continuous where $S_C$ is a whole multiple of $U$, so no tolerance is added to $\lfloor S_C/U\rfloor$.
- $E^{UB}$ is only defined for $0\le S_C\le n^CU$. Outside that range the cell computation returns an error instead of a number; $S_C=n^CU$ is allowed. With $n^C=0$ the bills must be empty, and a cap, if given, must be positive.
- Tasks whose cost or time is $\mathrm{NA}$ are left out of that dimension's samples; $p_K$ still uses the full $n^1$ and $n^0$. The "layer pool" means the arm's raw samples from all positive-weight cells in the layer, winsorized together. The library takes this pooled mean from the caller; building it from the local database is planned. Cancellation time $\bar T_C$ uses the same fallback chain as cost.
- If $f>0$ but $Q^{\rm match}$ is missing, quality is $\mathrm{NA}$; if $f=0$, $\bar Q_F=Q^{\ast}_{F,u}$.
- Overflow or NaN returns an error. If any dimension is $\mathrm{NA}$, the robust-comparison flag is off.

</details>

### 5. Aggregation and the value score $V$

*Spec §4 and §7. Implemented.*

Cell values are averaged with the cell weights, then layer values with the layer weights:

$$
C^*_{i,d}=\sum_{u\in d^+}\omega_uC^*_{i,u},\qquad
C^*_i=\sum_{d\in D^+}\omega_dC^*_{i,d},
$$

and the same for $T^{\ast}$ and $Q^{\ast}$. $d^+$ and $D^+$ are the cells and layers with positive weight. If any positive-weight cell is $\mathrm{NA}$, the aggregate is $\mathrm{NA}$; the remaining cells are not re-weighted.

The value score compares an arm with the reference anchor on all three dimensions at once:

$$
V_{i,d}=100
\left(\frac{C^{\rm ref}_{R,d}+c_0}{C^*_{i,d}+c_0}\right)^{a}
\left(\frac{T^{\rm ref}_{R,d}+t_0}{T^*_{i,d}+t_0}\right)^{b}
\left(\frac{1-Q^{\rm ref}_{R,d}+\epsilon}{1-Q^*_{i,d}+\epsilon}\right)^{g}.
$$

- $V=100$ means as good as the reference overall. Above 100 is better, below 100 is worse.
- Each factor is a ratio "reference over this arm", so lower cost, shorter time and fewer defects ($1-Q^{\ast}$) all raise $V$.
- $a$, $b$, $g$ are your weights for cost, time and quality, from the current weight version. They sum to 1 and each is at least $0.05$; without any evidence they are $1/3$ each.
- $c_0=0.15$ RMB, $t_0=15$ s and $\epsilon=0.02$ are noise floors. They keep tiny differences between very cheap, very fast or near-perfect arms from producing huge ratios.
- $(C^{\rm ref},T^{\rm ref},Q^{\rm ref})$ is the reference anchor. The library certifies it for each layer; see below.

The overall $V_i$ uses the same formula with overall values. A per-cell $V_{iu}$ is a diagnostic only. $V$ is computed in log space and requires $c_0,t_0,\epsilon>0$, $C,T\ge0$ and $Q\in[0,1]$.

**Reference anchor.** The reference arm's cells get the same $\mathrm{miss}$, $\mathrm{fail}$ and $\mathrm{valid}$ states, and the same $F_{\rm thin}$ and $F_{\rm bad}$ kinds, as any other arm. Each layer is then put in one tier, checked in the order III, II, I; the first that applies wins.

- **Tier III, not certified.** The reference has a $\mathrm{miss}$ cell, an $F_{\rm bad}$ cell, a positive-weight cell with an $\mathrm{NA}$ dimension, or fewer than 3 $\mathrm{valid}$ cells. Every reason that applies is listed, in this order. The layer has no $V_{i,d}$, but each arm's $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$ and the official frontier of section 8 are still produced. Because tier III is checked first, a layer with fewer than 3 positive-weight cells is never certified, even when every cell is valid.
- **Tier II, clamped.** Not tier III, and the reference has at least one $F_{\rm thin}$ cell. The layer anchor is clamped; the cell values are not:

$$
X^{\rm anchor}_{R,d}={\rm clip}\bigl(A_X,\ (1-\theta)a_X,\ (1+\theta)a_X\bigr),\qquad
A_X=\sum_{u\in d^+}\omega_uX^*_{R,u},\qquad
a_X=\frac{\sum_{u\in{\rm valid}}\omega_uX^*_{R,u}}{\sum_{u\in{\rm valid}}\omega_u},\qquad
\theta=0.10 .
$$

$X$ stands for cost and for time. Quality is clamped through its defect $D=1-Q$, with $A_D$ and $a_D$ defined the same way on $D$: $Q_{R,d}^{\rm anchor}=1-{\rm clip}\bigl(A_D,(1-\theta)a_D,(1+\theta)a_D\bigr)$. A layer value inside the window is used as it is; only a value outside it is pulled back to the edge. The cell values never change, so the fallback terms of section 4 still use the measured values. The library keeps $A$, $a$, the clamped value and the weight of the reference's $F_{\rm thin}$ cells for the header. In tier II a pure-route arm (one whose cells equal the reference's) need not score 100, and the legend has to say so.

- **Tier I, certified.** Every other case. The anchor is the reference's measured layer values, and a pure-route arm scores exactly 100.

$V$ is never divided by the pure-route arm's score to bring it back to 100; that would cancel the clamp and undo tier II.

### 6. Eligibility gates $\mathcal E_d$

*Spec §5. Implemented. Authenticity and log integrity are passed in by the caller until probes exist (step 13).*

An arm takes part in a layer's main comparison, the set $\mathcal E_d$, only if it passes all five gates:

1. **Complete case.** The arm has no $\mathrm{miss}$ and no $\mathrm{fail}$ cell in the layer: $N_{i,d}^{\rm miss}=0$ and $N_{i,d}^{\rm fail}=0$.
2. **Sample size.** $U_d$ is the number of positive-weight cells in the layer:

$$
n^{\rm comp}_{i,d}=\sum_{u\in d^+}\bigl(n^1_{iu}+n^0_{iu}\bigr)\ge\max(30,\,3U_d).
$$

3. **Cancellation and completion rates.** $N^{\rm sub}_{i,d}$ is the number of tasks submitted to the arm in the layer:

$$
\frac{n^C_{i,d}}{N^{\rm sub}_{i,d}}\le0.20,\qquad
\frac{n^1_{i,d}+n^0_{i,d}}{N^{\rm sub}_{i,d}}\ge0.50 .
$$

4. **Authenticity and log integrity.** Authenticity grades A, A− and B pass and C fails; an arm that passes on grade B is flagged "authenticity undetermined". The log-integrity check must pass. For now both results are passed in by the caller, so these two gates are only as reliable as the caller's input.
5. **Reference base.** In every positive-weight cell of the layer, the reference arm's $C^{\ast}$, $T^{\ast}$ and $Q^{\ast}$ are all defined (not $\mathrm{NA}$). The reference arm having no $\mathrm{miss}$ cell is necessary but not enough, since a cell with samples can still end up $\mathrm{NA}$.

The result lists whether the arm is eligible, every gate it failed, and the "authenticity undetermined" flag.

<details>
<summary>Implementation notes</summary>

- The rate gates are compared in integers, $5n^C\le N^{\rm sub}$ and $2n^{\rm comp}\ge N^{\rm sub}$, so there is no rounding at the boundaries. If $N^{\rm sub}=0$, both rate gates fail without dividing.
- All five gates are always evaluated, and every failed gate is listed once, in a fixed order; the check does not stop at the first failure.
- $n^C$ and $N^{\rm sub}$ are summed over the positive-weight cells that have data; a $\mathrm{miss}$ cell adds 0. In a layer with a $\mathrm{miss}$ cell the failure list may therefore also name a rate gate. The arm is ineligible either way, through the complete-case gate.
- The reference-base gate takes the reference arm's triples from the caller and does not check that they were really computed from the reference arm. The same cell given twice is an error.
- For attribution, each failure label maps to one of the five gates, and the primary failures leave out the rate gates when the complete-case gate has already failed (step 9).
- Step 9 checks the gates against the specification's worked example 6.

</details>

### 7. Diagnostic table

*Spec §5. Implemented as library functions; the table itself is not displayed yet.*

An arm with a $\mathrm{miss}$ or $\mathrm{fail}$ cell in a layer is not in $\mathcal E_d$. The diagnostic table gives such an arm a separate, labelled estimate that is never mixed into the main results.

**Admission.** The arm gets a diagnostic score only if it has at least one $\mathrm{valid}$ cell and

$$
{\rm ESS}_{i,d}=n^{\rm comp}_{i,d}+\kappa_{\rm eff}\ge3 .
$$

Here $\kappa_{\rm eff}$ is the prior strength $\kappa_{iu}$ from section 3, averaged over all of the layer's positive-weight cells with weights $\omega_u$. The prior comes from the caller; a cell for which no prior is passed in counts as $\kappa=0$, and its weight stays in the denominator. With the Jeffreys prior $\kappa=0$, so $\mathrm{ESS}$ is just $n^{\rm comp}$.

**Filling the gaps.** Fill values are built from the plain $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$ of the arm's own $\mathrm{valid}$ cells in the layer, never from the cancel-robust versions.

- A $\mathrm{miss}$ cell gets two values. The optimistic one is the weighted median of the valid cells, taken separately for cost, time and quality. It is the lower median: the valid cells' weights are normalized, values are sorted, and the first one whose cumulative weight reaches one half wins. Quality is sorted by its defect $1-Q$, so a tie always picks the better real cell. The conservative one is the worst valid value in each dimension: the highest cost, the highest time and the lowest quality, which may come from different cells. 
- A $\mathrm{fail}$ cell gets one value: in each dimension, the worse of its measured value and the worst valid value.

The filled cells are aggregated over the layer as in section 5, once with every $\mathrm{miss}$ cell at its optimistic value and once at its conservative value; $\mathrm{fail}$ cells use their single value both times. An arm with $\mathrm{miss}$ cells is shown with the range $[V^{\rm diag,cons},V^{\rm diag,opt}]$; an arm with only $\mathrm{fail}$ cells gets a single $V^{\rm diag}$, labelled with the total weight of its failed cells $w^{\rm fail}_{i,d}$. $V$ is computed against the diagnostic reference anchor, which does not depend on the tier and is never clamped. Following line 570 of the spec, the worst valid value is taken in each dimension on its own: the highest cost, the highest time and the lowest quality among the reference's positive-weight $\mathrm{valid}$ cells, which may come from different cells. A valid cell with an $\mathrm{NA}$ dimension still counts for its other dimensions. In every positive-weight cell, each dimension of the reference is then the worse of its own value and that worst valid value; an $\mathrm{NA}$ dimension takes the worst valid value. A library function computes this anchor and the caller passes it to the row. If some dimension is missing in every positive-weight valid cell of the reference, there is no anchor, and the row has no $V$.

**Order.** Rows are sorted by valid-cell coverage, highest first, then by model name:

$$
{\rm cov}_{i,d}:=1-w^{\rm miss}_{i,d}-w^{\rm fail}_{i,d}
$$

There is no way to sort by diagnostic $V$.

<details>
<summary>Implementation notes</summary>

- The layer aggregates in a diagnostic row are computed after filling. A raw measured layer mean, such as the $A_C=0.165$ in the specification's example 8, has to be computed by the caller from the measured values; the library does not store it.
- When an arm is not admitted, the row lists every reason that applies: $\mathrm{ESS}<3$ (`EssBelowThree`), no $\mathrm{valid}$ cell (`NoValidCell`), or a valid cell without a complete $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$, so no fill values can be built (`ValidCellIncomplete`). The list is empty exactly when the row is admitted.
- Bad input returns an error: the same cell given twice, a weight or $\kappa$ that is not a finite number, or a $\mathrm{fail}$ cell without a complete measured triple.
- Checked against the specification's examples: the $F_{\rm thin}$ cell and diagnostic aggregate 0.7875 in example 6, and the $F_{\rm bad}$ cell and 0.9000 in example 7. The raw mean 0.1650 in example 8 is checked by the tests for steps 6 and 7.

</details>

### 8. Frontier

*Spec §6. Implemented as library functions; the display is planned.*

Among the eligible arms of a layer, arm $j$ dominates arm $i$ when it is at least as good in every dimension (cost and time no higher, quality no lower) and strictly better in at least one. The weights $a$, $b$, $g$ play no part here.

$$
\mathcal F^{\rm off}_d=\{i\in\mathcal E_d:\nexists j\in\mathcal E_d,\ j\succ_{\rm off}i\},\qquad
\mathcal F^{\rm rob}_d=\{i\in\mathcal E_d:\nexists j\in\mathcal E_d,\ j\succ_{\rm rob}i\}
$$

$$
\mathcal F^{\rm rec}_d=\{i\in\mathcal F^{\rm off}_d:\nexists j\in\mathcal E_d,\ j\succ_{\rm rob}i\}
$$

$\succ_{\rm off}$ compares the plain $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$ and $\succ_{\rm rob}$ the cancel-robust versions. $\mathcal F^{\rm off}_d$ is the official frontier, $\mathcal F^{\rm rob}_d$ the cancel-robust frontier, and $\mathcal F^{\rm rec}_d$ the recommended set: official frontier arms that no arm beats on the cancel-robust values.

**Official pick per layer.** In a tier I or tier II layer, the official first choice is every arm tied for the highest $V_{i,d}$ within $\mathcal F^{\rm rec}_d$. A tier III layer still gets its frontiers but no first choice. When $\mathcal F^{\rm rec}_d$ is empty, nobody is first: $\mathcal F^{\rm rob}_d$ is listed separately as the "cancel-robust alternative" and is never labelled the official first choice.

**Bill too coarse.** If any positive-weight cell of an arm has cancellation bills that give only a lower bound (section 4), cancel-robust dominance is turned off for the arm in that layer and it is tagged "bill too coarse". The tag shows on every row of that arm in that layer, whether or not the arm is in $\mathcal F^{\rm rec}_d$. A tagged arm can still be the official first choice and keeps its tag.

**Overall.** An arm's overall $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$ are computed and reported whenever all of its positive-weight layer values are defined, together with its valid coverage:

$$
{\rm cov}_i=\sum_{d\in D^+}\omega_d\bigl(1-w^{\rm miss}_{i,d}-w^{\rm fail}_{i,d}\bigr).
$$

An arm with ${\rm cov}_i<1$ is marked as not fully covered: it is a diagnostic only, with no frontier, recommendation or rank. The overall reference anchor adds up the layer anchors with the layer weights:

$$
X^{\rm ref}_R=\sum_{d\in D^+}\omega_dX^{\rm anchor}_{R,d},\qquad X\in\{C,T,Q\}.
$$

If any positive-weight layer is tier III, the overall reference is $\mathrm{NA}$: there is no overall $V$ and no overall recommendation, only each arm's overall values and the per-layer frontiers, under a header saying the reference is not certified in some layers. That is then the only header shown; only when the overall reference is certified and some positive-weight layer is tier II does the header say the reference includes a clamped tier II layer. An arm gets an overall $V_i$ only if the overall reference is certified and the arm passes every gate over the whole grid: $n_i^{\rm comp}\ge\max(30,3U_i)$, no $\mathrm{miss}$ or $\mathrm{fail}$ cell, cancellations at most 20%, completions at least 50%, authenticity passed, and a place on the official frontier of at least one positive-weight layer ($i\in\mathcal F^{\rm all}$). The overall first choice is every arm tied for the highest overall $V_i$.

<details>
<summary>Implementation notes</summary>

- The caller passes in the arms of $\mathcal E_d$; the frontier function does not check eligibility itself.
- An arm whose plain triple has an $\mathrm{NA}$ is left out of $\mathcal F^{\rm off}_d$; an arm whose cancel-robust triple has an $\mathrm{NA}$ is left out of $\mathcal F^{\rm rob}_d$ and of every cancel-robust comparison.
- The cancel-robust values are $\mathrm{NA}$ exactly where the plain values are, so with the library's own values an arm on the official frontier always has a complete cancel-robust triple. The specification does not cover a caller that passes in such an arm without one; the library then leaves it out of $\mathcal F^{\rm rec}_d$, in every build.
- An arm tagged "bill too coarse" is left out of $\mathcal F^{\rm rob}_d$ and of every cancel-robust comparison, in both directions. If it is on the official frontier, nothing can beat it on the cancel-robust values, so it stays in $\mathcal F^{\rm rec}_d$.
- The tag is decided by the cell note. The frontier result also lists the tagged arms in $\mathcal F^{\rm rec}_d$, meaning those whose robust dominance is switched off. That switch comes from each cell's robust-dominance flag, not from the notes; for the arms of $\mathcal E_d$ it agrees with the notes. An arm whose dominance is off only because its cancel-robust triple has an $\mathrm{NA}$ is not tagged; it is left out of $\mathcal F^{\rm rec}_d$ as described above.
- A separate helper returns every arm tied for the highest $V$; the tests check that these arms are on the official frontier.
- The tests check the specification's examples 6, 7 and 8 end to end, from $\mathcal E_d$ through the frontier to the official pick.

</details>

### 9. Weights from your answers and behaviour

*Spec §10. Implemented as library functions; the question screen and the suggestion display are planned.*

**Self-report.** You answer pairwise questions. Each question offers two options, A and B, that differ in two of cost, time and quality, pulling in opposite directions; the third dimension is equal. With the noise floors $c_0$, $t_0$, $\epsilon$, the ratios between A and B are $\rho_C=(C_A+c_0)/(C_B+c_0)$, $\rho_T=(T_A+t_0)/(T_B+t_0)$ and $\rho_D=(1-Q_A+\epsilon)/(1-Q_B+\epsilon)$; the equal dimension has ratio 1.

$$
s(w)=a\ln\rho_C+b\ln\rho_T+g\ln\rho_D,\qquad
\Delta=\tfrac12\bigl(|\ln\rho_C|+|\ln\rho_T|+|\ln\rho_D|\bigr),\qquad
u(w)=-s(w)/\Delta .
$$

You answer on five levels: strongly A ($+2$), slightly A ($+1$), about the same ($0$), slightly B ($-1$), strongly B ($-2$). A weight $w$ predicts the level

$$
\ell(w)=\begin{cases}
+2, & u(w)>\delta_2,\\
+1, & \delta_1<u(w)\le\delta_2,\\
0, & |u(w)|\le\delta_1,\\
-1, & -\delta_2\le u(w)<-\delta_1,\\
-2, & u(w)<-\delta_2,
\end{cases}\qquad \delta_1=0.25,\ \delta_2=0.50 .
$$

$u$ is rounded to 9 decimal places first, so a value that is exactly $0.25$ or $0.50$ stays inside the closed band.

- Candidate weights are a grid: $a$, $b$, $g$ each from $0.05$ to $0.90$ in steps of $0.05$, with $a+b+g=1$. That gives 171 points.
- The question bank has 96 questions: 3 pairs of dimensions, ratios from $\{1.3, 1.5, 2, 3\}$ for each of the two dimensions, and 2 directions.
- Let $m$ be the smallest total level deviation from your answers over the grid. The core region $\Omega^{\rm core}_s$ holds the grid points with deviation $m$. The weight region $\Omega_s$ holds the points with deviation at most $m+1$, so one answer may be a level off.
- The next question is the unanswered one that splits $\Omega_s$ most evenly over the five levels; on a tie, the first one in the bank. Questions stop once at least 4 are answered and $\Omega_s$ has at most 15 points, or after 10 questions.
- The result is shown mainly as $\Omega_s$ and its range in each dimension. The default point $w_s$ is the grid point nearest the centroid of the core region.
- If $m\ge2$, the answers are flagged as contradicting each other, with a list of the questions that are two or more levels off at some core point. The list may be empty.
- The amounts in a question are computed one dimension at a time from a base value: the previous window's median (in the first window, 0.5 RMB and 120 s) for cost and time, and a defect rate passed in by the caller for quality. In each dimension, the option that is better in that dimension shows the base, and the worse option's value is computed from the ratio, so cost and time stay positive and a defect rate is never negative; in the equal dimension both show the base. A question whose computed defect rate would go above 1 is skipped, not clamped.
- The cut points $0.25$ and $0.50$ and the "one answer may be a level off" error model have been checked in simulation only, not with real users.

**Behaviour suggestions.** These use the choice events of the previous window: which arms you could pick and which one you picked. Only real choices between at least 2 arms count, and an event is dropped whole if any of its arms lacks values. Using the noise floors $c_0$, $t_0$, $\epsilon$ from the constants table and the arm's cell values from the previous window, set $x_l=\bigl(\ln(C^{\ast}_l+c_0),\ \ln(T^{\ast}_l+t_0),\ \ln(1-Q^{\ast}_l+\epsilon)\bigr)$, a choice model is fitted with $\vartheta\ge0$:

$$
\Pr(j_e\mid S_e;\vartheta)=\frac{\exp(-\vartheta\cdot x_{j_e})}{\sum_{l\in S_e}\exp(-\vartheta\cdot x_l)},\qquad
w_b=\frac{\hat\vartheta}{\lVert\hat\vartheta\rVert_1},\qquad \hat\lambda=\lVert\hat\vartheta\rVert_1 .
$$

- There is no signal, and so no suggestion, when there are fewer than 20 events, fewer than 2 different arms were ever picked, the differences $x_{j_e}-x_l$ between the chosen arm and each other option have rank below 3, $\hat\lambda<0.5$, or the fit has no well-defined maximum.
- Otherwise the support set $\Omega_b$ holds the grid points whose profile log-likelihood is within $3.0$ of the maximum. It is a heuristic set, not a confidence region, and it can be empty: $w_b$ may lie off the grid (a component below 0.05), or the likelihood may be so sharp that even the nearest grid point is more than 3.0 below the maximum. An empty $\Omega_b$ is not an error: with a self-report there is then no overlap, so the "no overlap" row below applies; without one, the "no self-report, behaviour signal" row applies.
- 200 bootstrap resamples report the 5%, 50% and 95% points of $w_b$. If fewer than 100 resamples succeed, the result is marked unstable.
- The first window gives no signal. Behaviour from the window being scored, or from a later one, is refused with an error.

**How they combine.** Behaviour never changes the weights. The weights in effect are $w_s$ if you have answered the questions, otherwise $1/3$ each. Behaviour only adds a suggestion:

| Situation | Weights in effect | Suggestion |
|---|---|---|
| Self-report, no behaviour signal | $w_s$ | none |
| Self-report, $\Omega_s$ and $\Omega_b$ overlap | $w_s$ | "behaviour agrees", plus the point of $\Omega_s$ with the highest profile log-likelihood |
| Self-report, no overlap | $w_s$ | the conflict, plus $w_b$ |
| No self-report, behaviour signal | $1/3$ each | $w_b$ |
| Neither | $1/3$ each | a prompt to answer the questions |

<details>
<summary>Implementation notes</summary>

- On a tie for nearest grid point, $w_s$ is the point with the smallest $(a, b)$ in dictionary order. The distance is compared exactly in integers.
- The core works with ratios only. Turning a question into amounts and times is done from the ratios; a question that cannot be shown is skipped and not asked again. A defect base of 0 is valid.
- The rank is computed by elimination with full pivoting and a relative tolerance of $10^{-9}$. "Different arms" are counted by arm id.
- $\hat\vartheta$ is found by projected Newton steps on $\vartheta\ge0$. Before the fit, the events are checked for separation. The data are separable if some direction $d\ge0$, $d\ne0$, makes the chosen arm $j_e$ of every event no worse than each other option $l$: $d\cdot(x_l-x_{j_e})\ge0$. The fit only runs when the differences have rank 3, so at least one of these is then strict, so the likelihood keeps rising along $\vartheta+td$ and has no maximum: separable data are not identifiable. Separable data, a nearly singular information matrix, or no convergence give "no signal: not identifiable" instead of an error.
- A bootstrap resample that fails (difference rank below 3, separable, or no finite fit with $\lVert\vartheta\rVert_1>0$) is skipped and counted. The random seed is passed in by the caller.
- The profile log-likelihood of $w$ is the maximum of the log-likelihood $L$ along $\lambda w$ for $\lambda>0$. If $L$ does not increase from $\lambda=0$, the value at $\lambda=0$ is used. The threshold for $\Omega_b$ uses $L(\hat\vartheta)$ directly; $w_b$ is not moved onto the grid.

</details>

### 10. Planned

These parts of the specification are not implemented:

- **The rest of step 10**: showing the old and new results side by side when $R_k$ or $F_k$ changes (§13), the hint after failures that you usually switch to some arm X and might make X the fallback (§13), and descriptive totals across scoring scopes (§6). Showing the first window's frontier labelled as not yet calibrated is also planned.
- **Weight sensitivity** (step 12): how results change across the weight grid, plus a check that halves and doubles each noise floor one at a time.
- **Probes and relay checks** (step 13): a fixed probe set that checks whether a route serves the model it claims, plus log-integrity checks.

### Constants

| Constant | Value | Meaning |
|---|---|---|
| $K$ | 3 | Maximum attempts per arm before handoff (worst case $2K=6$ per task) |
| $\pi$ | passed in by the caller (estimation from data is planned) | Success probability of a retry after a failure |
| $\kappa_p$ | $20\rho$ | Strength of the pass-rate prior |
| $\kappa_C$ | 5 | Shrinkage of cost and time toward the benchmark |
| $\kappa_F$ | 5 | Shrinkage of fallback quality |
| $m_0$ | 0.5 | Shrinkage of the quality score |
| $c_0$ | 0.15 RMB | Cost noise floor |
| $t_0$ | 15 s | Time noise floor |
| $\epsilon$ | 0.02 | Quality defect noise floor |
| $\theta$ | 0.10 | Tier II clamp window for the reference anchor (cost, time and defects) |
| $a,b,g$ | $1/3$ each by default, each $\ge0.05$ | Weights of cost, time and quality in $V$ |
| Answer cut points $\delta_1,\delta_2$ | 0.25, 0.50 | Bands of $u(w)$ for the five answer levels |
| Weight grid | step 0.05, each weight 0.05 to 0.90, 171 points | Candidate weights for the questions and for $\Omega_b$ |
| Question stop rule | at least 4 answers and $\lvert\Omega_s\rvert\le15$ points, or 10 answers | When the questions stop |
| Behaviour signal | at least 20 events, at least 2 different chosen arms, difference rank 3, $\hat\lambda\ge0.5$ | Otherwise there is no behaviour suggestion |
| Support set | within 3.0 of the maximum profile log-likelihood; 200 bootstrap resamples | $\Omega_b$ and the spread of $w_b$ |
| Time window | 30 days | Weights and references are fixed from the previous window |
| Redo window | 30 minutes | A redo request in this period marks the task failed |
| Rollback window | 24 hours | A full revert in this period marks a coding task failed |
| Spot rating | 10%, at most 3 per day | Share of successful tasks selected for a rating request (the prompt itself is planned) |
| Eligibility thresholds | $n^{\rm comp}\ge\max(30,3U_d)$, cancellations $\le20\%$, completions $\ge50\%$ | Gates 2 and 3 for $\mathcal E_d$ |
| Diagnostic admission | $\mathrm{ESS}\ge3$ and at least one $\mathrm{valid}$ cell | Below this an arm gets no diagnostic score |
| Rating-mode threshold | $n_q\ge3$ per arm per cell | Otherwise the layer uses success-rate mode |
| Valid reference cells | at least 3 per layer | Fewer makes the layer tier III |
| Fallback rating threshold | $n_q\ge3$ in every positive-weight cell | Needed to keep a fallback arm other than $R_k$; defined separately from the rating-mode threshold |

## Usage

### For users

Planned. There is no app to install or run yet. The plan is a desktop app that stays in the system tray.

### For developers

Core and store crates (no system GUI libraries needed; a C compiler is needed for the bundled SQLite):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Web app (Node.js 20+):

```sh
cd web
npm ci
npm run lint
npm run typecheck
npm run build
```

Desktop shell: `src-tauri/` is a separate Cargo package so that the core can be tested without GUI libraries. Building it needs the Tauri 2 system prerequisites (on Linux: WebKitGTK 4.1 and related development packages). Build the web app first, then run `cargo build` from inside `src-tauri/`. CI installs these prerequisites and runs `cargo check` and `cargo clippy` for the shell. The shell has only been compile-checked so far; it has not been run or packaged.

## Layout

| Path | What it is |
|---|---|
| `crates/agent-value-core/` | Pure-Rust core library. All implemented formula logic and its tests live here. No dependencies. |
| `crates/agent-value-store/` | Local SQLite storage (bundled SQLite, no system library needed). |
| `src-tauri/` | Minimal Tauri 2 desktop shell that links the core crate. Not a member of the default Cargo workspace. |
| `web/` | React + TypeScript + Vite front end. Display only; it never computes scores. |

## Secrets and privacy

Real API keys go only in a local `.env` file, which is git-ignored. `.env.example` lists variable names only. agent-value stores the name of the environment variable a route uses, never its value; the database has no column for key values.

All data stays in one local SQLite file. Prompt and code text is never stored: only a category, a length and an HMAC-SHA256 digest whose key is kept outside the database. Task-level data is purged after 180 days, and the store can export every row or delete everything.

## License

Apache-2.0. See `LICENSE` and `THIRD_PARTY_NOTICES`.
