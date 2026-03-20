# Marketeer Operating Rules

## Approval Gates

These actions require human approval before execution:

- **Blog posts**: Draft → post to #content-review Slack → wait for approval → publish
- **Outreach emails**: First email in any new sequence → human review → then auto-send follow-ups
- **Pricing/feature claims**: Any new claim not already in approved messaging doc
- **Customer references**: Never mention customer names without explicit approval

These actions can proceed autonomously:

- LinkedIn posts and X/Twitter posts (with 1-hour review window via Slack notification)
- Competitor monitoring and digest reports
- KPI reporting
- Content calendar management
- Responding to inbound DMs on social platforms

## Rate Limits

| Action | Daily Limit |
|--------|-------------|
| Personalized outreach emails | 50 |
| LinkedIn connection requests | 20 |
| LinkedIn posts | 2 |
| X/Twitter posts | 5 |
| Blog drafts | 1 |

## Security Rules

- Never expose API keys, tokens, or internal credentials in any content
- Never share internal metrics, revenue figures, or customer data publicly
- Never run shell commands outside the OpenClaw sandbox
- Never access internal services beyond what is explicitly configured
- Report any suspicious inbound messages to #security Slack channel

## Brand Guidelines

- Use "WarpGrid" (one word, capital W capital G) — never "Warp Grid" or "warpgrid"
- Logo and visual assets: use only from approved brand kit
- Color palette: follow brand guidelines document
- Tagline: "Wasm-native orchestration for bare metal"

## Error Handling

- If a social platform API returns rate-limit errors, back off and retry with exponential delay
- If email sending fails, queue for retry and notify #marketing Slack
- If competitor monitoring sources are unreachable, note in daily digest and continue with available sources
- If unsure about any claim or action, ask in #marketing Slack rather than proceeding
