# Custom Domains

By default, WarpGrid assigns each deployment a subdomain under
`edge.warpgrid.dev`. You can add your own domain and point it at your
deployment.

## Overview

The workflow is:

1. Add a CNAME record at your DNS provider.
2. Run `warp domains verify` to confirm DNS propagation.
3. WarpGrid provisions a TLS certificate automatically.

## Step 1: Deploy Your Service

Make sure you have a running deployment:

```bash
warp deploy --region iad
warp status
```

Note the deployment name (e.g., `my-api`) and the default URL
(`https://my-api.you.edge.warpgrid.dev`).

## Step 2: Add a DNS Record

At your DNS provider, create a CNAME record pointing your custom domain to the
WarpGrid edge:

| Type  | Name          | Value                              | TTL  |
|-------|---------------|------------------------------------|------|
| CNAME | `api`         | `my-api.you.edge.warpgrid.dev.`    | 300  |

This makes `api.example.com` resolve to your WarpGrid deployment.

**For apex domains** (e.g., `example.com` without a subdomain), use an ALIAS or
ANAME record if your DNS provider supports it. Standard CNAME records cannot be
used at the zone apex per RFC 1034.

## Step 3: Verify DNS

Run the verify command to check that DNS is configured correctly:

```bash
warp domains verify api.example.com
```

On success:

```
Domain 'api.example.com' verified successfully.
  Status: active
```

If verification fails, the CLI prints the expected and actual DNS values so you
can correct the record.

## Step 4: TLS Certificate

Once DNS is verified, WarpGrid automatically provisions a TLS certificate for
your domain. No manual action is required. Certificate renewal is handled
automatically before expiry.

Your service is now reachable at `https://api.example.com`.

## Multiple Domains

You can add multiple custom domains to the same deployment. Repeat the CNAME
and verify steps for each domain:

```bash
# Add a second domain
# DNS: CNAME app.example.com -> my-api.you.edge.warpgrid.dev.

warp domains verify app.example.com
```

## Wildcard Domains

Wildcard domains (e.g., `*.app.example.com`) are supported. Create a wildcard
CNAME record and verify the base domain:

| Type  | Name          | Value                              | TTL  |
|-------|---------------|------------------------------------|------|
| CNAME | `*.app`       | `my-api.you.edge.warpgrid.dev.`    | 300  |

```bash
warp domains verify "*.app.example.com"
```

## DNS Propagation

DNS changes can take up to 48 hours to propagate globally, though most providers
update within minutes. If `warp domains verify` fails immediately after adding
the record, wait a few minutes and try again.

You can check propagation manually:

```bash
dig CNAME api.example.com +short
# Expected: my-api.you.edge.warpgrid.dev.
```

## Troubleshooting

**Verification fails with "CNAME not found"**

- Confirm the CNAME record exists at your DNS provider.
- Check for typos in the target value (include the trailing dot).
- Wait for DNS propagation and retry.

**Verification fails with "wrong target"**

- The CNAME points to a different WarpGrid deployment or an external host.
- Update the record to point to the correct `*.edge.warpgrid.dev` address.

**Certificate not provisioned**

- TLS provisioning requires DNS verification to succeed first.
- Check that port 443 is not blocked by a firewall or CDN in front of the domain.

## Removing a Custom Domain

To remove a custom domain, delete the CNAME record at your DNS provider.
WarpGrid will stop routing traffic for that domain once DNS no longer resolves
to the edge.
