# Active Projects & Systems

_MEMORY.md has one-line pointers; this is the detail._

## The Model Mind (Mason's real body of work)
10-year body-of-work: mental models, *"building codes for a mind you like living in"* — calm + sharp + present. Triad: calm (Stoic/Tao) + sharp (models) + present (Buddhist).
- **THE PRODUCT, FOUND (June 20, 2026):** Definitive body of work (Seth Godin's daily-blog model) on thinking clearly by stacking mental models, synthesizing Christianity/Buddhism/Taoism/Stoicism/modern philosophy. Founding metaphor: *building codes for a mind you'd actually want to live in.* Distribution = build-in-public documenting HOW he thinks. Compounding ladder: content → weekly newsletter → free PDF → YouTube → keynote → book → kids' game.
- **The discipline trap to guard:** ship the daily content for 90 days FIRST; don't build the upper ladder rungs prematurely (his core weakness is spreading across too many ambitions). This is the first idea he argued FOR, not against.

## The Product-Idea Pattern (key insight)
Mason cycled ~6 standalone product ideas in weeks — **every one sparked then evaporated.** The only things that HOLD his energy are tied to his identity/life: **The Model Mind (body of work)** and **the Stan role.** He's not a "build a SaaS product" guy — he's a **"build a body of work + own a high-leverage role"** guy. Filter: identity-linked sticks, abstract standalone dies.

## masonlee.build — the builder-site (LIVE)
His public build-in-public portfolio + product storefront. Shipped live July 15 2026, major v2 July 16.
- **Stack:** Next.js 15 · Vercel (auto-deploys from GitHub) · Supabase · Stripe LIVE · Resend (email).
- **Repo:** `masonleetompkins/masonleebuild` (PRIVATE).
- **⚠️ Deploy golden rule:** commit as `Mason Tompkins <mason.tompkins@gmail.com>` or Vercel skips the auto-deploy.
- **assistant@masonlee.build is MY email address** — sender for site notifications. Mason gave me my own address on his domain.

## Content
- **Revenue path:** ELI5 video → "comment MODELS" CTA → ManyChat auto-DM → Stan Store ($10 mental models doc). CTA = 4-10x better results.
- **Mission (his north star):** *"I must find a way to use my content and followers to make enough money to aggressively pay off my tax debt. There is nothing else that I could do that would make me feel better in my day to day life."* Salary alone won't clear it.
- ELI5 style: cut rhythm 16-25/min, Impact ALL-CAPS captions, -14 LUFS audio, 3 hook types. Sony a7III, Slog2, 4K 23.98fps. Last full take = best take.

## AYGENT (the app this memory lives inside)
See MEMORY.md for the full story. Repo: github.com/masonleetompkins/aygent. Build rhythm: Mason runs on Mac, I author + push, he pastes what breaks, I fix at the seam.
- **Pro Mode (self-hosting):** shell.exec via a privileged Rust exec broker — only Rust spawns, cwd pinned to the jail, env scrubbed (broker token + API keys never inherited by children), GUI-launch denylist so the agent can never rebuild its own host. shell_run (one-shot digest) + shell_spawn/poll/kill (long processes).
- **Config lives in the ROOT FOLDER** (own-your-agent), not Application Support. One pointer (root.json) + keys (Keychain) stay outside. Each agent gets a home subfolder `<root>/<Name>/`.
- **Self-hosted build loop:** stable AYGENT.app hosts a Dev agent editing a staging checkout; promote.sh does staging→production with rollback. DOGFOOD.md has the whole flow.
