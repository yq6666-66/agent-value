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
| 6-7 | Cell cost, time and quality $C^{\ast}, T^{\ast}, Q^{\ast}$; aggregation; value score $V$ | Implemented ($V$ is correct only when the reference anchor is supplied; see step 10) |
| 8 | Eligibility gates $\mathcal E_d$ | Implemented (authenticity and log integrity are passed in by the caller until step 13) |
| 9 | Frontier and diagnostic table | Implemented (per layer; the overall recommendation needs step 10, and the display is planned) |
| 10 | Reference anchor tiers I / II / III, fallback arm selection | Planned |
| 11 | Weight elicitation from your answers and behaviour | Planned |
| 12 | Weight sensitivity display | Planned |
| 13 | Probes and relay checks | Planned |

Changes by version are in [CHANGELOG.md](CHANGELOG.md).

## Key ideas

- **Scoring scope.** Each task type is scored separately. By default there are two scopes: coding and non-coding. Scores are never combined across scopes.
- **Arm.** The unit being compared: a model plus a route. A route is the API host, the protocol (for example Anthropic Messages or OpenAI Chat Completions) and the agent or client. If any of the three differs, it is a different arm. Changing the API key on the same route does not make a new arm.
- **Cell and layer.** Tasks are grouped into cells $u$ by subtype (for coding by default: fix, add, refactor, explain and review, other). By default there is one layer; optionally, tasks can be split into layers $d$ by size (small, medium, large), measured only on your own input, and a cell is then a subtype within a layer. Each cell has a weight $\omega_u$ equal to its share of the layer's tasks in the previous window.
- **Task.** One stretch from your submission to delivery, abandonment or cancellation.
- **Time window.** Scoring runs in 30-day windows. Cell weights, your weight version, price tables and classification rules for a window are fixed from earlier data when the window starts; the reference and fallback arms are frozen the same way once chosen (the rules for choosing them are step 10, planned). In the first window $V$ is withheld; the library already does this. By the specification the first window still shows each arm's cost, time and quality and the frontier, labelled as not yet calibrated. The library computes the frontier, but showing it with that label is planned.
- **Reference arm $R_k$ and fallback arm $F_k$.** $R_k$ is the arm every other arm is compared against. $F_k$ is the arm a task is handed to after an arm fails $K$ times; by default $F_k=R_k$. The reference and fallback arms do not themselves hand off to anyone.

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

*Spec §4. Implemented, with the reference anchor supplied by the caller until step 10.*

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
- $(C^{\rm ref},T^{\rm ref},Q^{\rm ref})$ is the reference anchor. Choosing it (tier I uses the measured reference values, tier II anchors bounded on both sides for each of cost, time and quality, tier III produces no $V$) is step 10 and is planned. Until then $V$ is correct only when the caller supplies the anchor.

The overall $V_i$ uses the same formula with overall values. A per-cell $V_{iu}$ is a diagnostic only. $V$ is computed in log space and requires $c_0,t_0,\epsilon>0$, $C,T\ge0$ and $Q\in[0,1]$.

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

The filled cells are aggregated over the layer as in section 5, once with every $\mathrm{miss}$ cell at its optimistic value and once at its conservative value; $\mathrm{fail}$ cells use their single value both times. An arm with $\mathrm{miss}$ cells is shown with the range $[V^{\rm diag,cons},V^{\rm diag,opt}]$; an arm with only $\mathrm{fail}$ cells gets a single $V^{\rm diag}$, labelled with the total weight of its failed cells $w^{\rm fail}_{i,d}$. $V$ is computed against a diagnostic reference that the caller passes in; without one, the row has no $V$.

**Order.** Rows are sorted by valid-cell coverage, highest first, then by model name:

$$
{\rm cov}_{i,d}:=1-w^{\rm miss}_{i,d}-w^{\rm fail}_{i,d}
$$

There is no way to sort by diagnostic $V$.

<details>
<summary>Implementation notes</summary>

- The layer aggregates in a diagnostic row are computed after filling. A raw measured layer mean, such as the $A_C=0.165$ in the specification's example 8, has to be computed by the caller from the measured values; the library does not store it.
- When an arm is not admitted, the row does not yet say which condition it missed.
- Bad input returns an error: the same cell given twice, a weight or $\kappa$ that is not a finite number, or a $\mathrm{fail}$ cell without a complete measured triple.
- Checked against the specification's examples: the $F_{\rm thin}$ cell and diagnostic aggregate 0.7875 in example 6, and the $F_{\rm bad}$ cell and 0.9000 in example 7. The raw mean 0.1650 in example 8 is checked by the tests for steps 6 and 7.

</details>

### 8. Frontier

*Spec §6. Implemented per layer. The overall recommendation depends on the anchor tiers (step 10) and is planned.*

Among the eligible arms of a layer, arm $j$ dominates arm $i$ when it is at least as good in every dimension (cost and time no higher, quality no lower) and strictly better in at least one. The weights $a$, $b$, $g$ play no part here.

$$
\mathcal F^{\rm off}_d=\{i\in\mathcal E_d:\nexists j\in\mathcal E_d,\ j\succ_{\rm off}i\},\qquad
\mathcal F^{\rm rob}_d=\{i\in\mathcal E_d:\nexists j\in\mathcal E_d,\ j\succ_{\rm rob}i\}
$$

$$
\mathcal F^{\rm rec}_d=\{i\in\mathcal F^{\rm off}_d:\nexists j\in\mathcal E_d,\ j\succ_{\rm rob}i\}
$$

$\succ_{\rm off}$ compares the plain $C^{\ast}$, $T^{\ast}$, $Q^{\ast}$ and $\succ_{\rm rob}$ the cancel-robust versions. $\mathcal F^{\rm off}_d$ is the official frontier, $\mathcal F^{\rm rob}_d$ the cancel-robust frontier, and $\mathcal F^{\rm rec}_d$ the recommended set: official frontier arms that no arm beats on the cancel-robust values.

<details>
<summary>Implementation notes</summary>

- The caller passes in the arms of $\mathcal E_d$; the frontier function does not check eligibility itself.
- An arm whose plain triple has an $\mathrm{NA}$ is left out of $\mathcal F^{\rm off}_d$; an arm whose cancel-robust triple has an $\mathrm{NA}$ is left out of $\mathcal F^{\rm rob}_d$ and of every cancel-robust comparison.
- The cancel-robust values are $\mathrm{NA}$ exactly where the plain values are, so with the library's own values an arm on the official frontier always has a complete cancel-robust triple. The specification does not cover a caller that passes in such an arm without one; the library then leaves it out of $\mathcal F^{\rm rec}_d$, in every build.
- When an arm's cancellation bills give only a lower bound, the specification turns off cancel-robust dominance for it and tags it "bill too coarse". Such an arm is left out of $\mathcal F^{\rm rob}_d$ and of every cancel-robust comparison, in both directions. If it is on the official frontier, nothing can beat it on the cancel-robust values, so it stays in $\mathcal F^{\rm rec}_d$, and the result also lists it as having a coarse bill. Showing that tag is planned (step 10).
- A separate helper returns every arm tied for the highest $V$; the tests check that these arms are on the official frontier.
- Not implemented yet: showing $\mathcal F^{\rm rob}_d$ as a "cancel-robust alternative" when $\mathcal F^{\rm rec}_d$ is empty (never labelled as the official first choice), the internal check that an overall recommendation sits on at least one layer's frontier, and the overall $V$ with its conditions, which need step 10.

</details>

### 9. Planned

These parts of the specification are not implemented:

- **Reference anchors and fallback arm selection** (step 10): the three anchor tiers and the rules for choosing $R_k$ and $F_k$. A fallback arm $F_k$ other than $R_k$ must, in the previous window, pass the authenticity check, have no $F_{\rm bad}$ cell, meet the tier I or tier II conditions, and have at least 3 ratings ($n_q\ge3$) in every cell; otherwise $F_k=R_k$, and the change is noted in the version record.
- **Weights** (step 11): estimating $a$, $b$, $g$ from pairwise questions and from your behaviour.
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
| $a,b,g$ | $1/3$ each by default, each $\ge0.05$ | Weights of cost, time and quality in $V$ |
| Time window | 30 days | Weights and references are fixed from the previous window |
| Redo window | 30 minutes | A redo request in this period marks the task failed |
| Rollback window | 24 hours | A full revert in this period marks a coding task failed |
| Spot rating | 10%, at most 3 per day | Share of successful tasks selected for a rating request (the prompt itself is planned) |
| Eligibility thresholds | $n^{\rm comp}\ge\max(30,3U_d)$, cancellations $\le20\%$, completions $\ge50\%$ | Gates 2 and 3 for $\mathcal E_d$ |
| Diagnostic admission | $\mathrm{ESS}\ge3$ and at least one $\mathrm{valid}$ cell | Below this an arm gets no diagnostic score |
| Rating-mode threshold | $n_q\ge3$ per arm per cell | Otherwise the layer uses success-rate mode |

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
