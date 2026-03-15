# Deploy to WarpGrid Action

A composite GitHub Action that builds and deploys a Wasm component to WarpGrid Cloud.

## Inputs

| Input | Description | Required | Default |
|-------|-------------|----------|---------|
| `api-key` | WarpGrid API key | Yes | — |
| `api-url` | WarpGrid Cloud API URL | No | `https://app.warpgrid.dev` |
| `region` | Target region (`iad`, `ams`, `sin`, `gru`, `syd`, or `all`) | No | `iad` |
| `lang` | Override build language (`rust`, `go`, `bun`, `typescript`) | No | — |
| `working-directory` | Project directory | No | `.` |

## Outputs

| Output | Description |
|--------|-------------|
| `deployment-url` | URL of the deployed application |

## Usage

```yaml
# .github/workflows/deploy.yml
name: Deploy to WarpGrid
on:
  push:
    branches: [main]
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/deploy
        with:
          api-key: ${{ secrets.WARPGRID_API_KEY }}
          region: iad
```

### Deploy to multiple regions

```yaml
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/deploy
        with:
          api-key: ${{ secrets.WARPGRID_API_KEY }}
          region: all
```

### Deploy a specific language project

```yaml
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/deploy
        with:
          api-key: ${{ secrets.WARPGRID_API_KEY }}
          lang: typescript
          working-directory: ./my-app
```
