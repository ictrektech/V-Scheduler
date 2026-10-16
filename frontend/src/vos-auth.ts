export interface VOSTokenSet {
  access_token: string
  token_type?: string
  expires_in?: number
  refresh_token?: string
  id_token?: string
  scope?: string
}

interface VOSOAuth2 {
  authorize(params: {
    client_id: string
    response_type: 'code'
    scope: string
    state: string
    code_challenge: string
    code_challenge_method: 'S256'
    nonce?: string
  }): Promise<{ code: string; state: string }>
  token(params: {
    grant_type: 'authorization_code'
    code: string
    code_verifier: string
    client_id: string
  }): Promise<VOSTokenSet>
}

declare global {
  interface Window {
    vos_platform?: {
      api?: {
        v1000?: {
          oauth2?: VOSOAuth2
        }
      }
    }
    __VOS_APP_CONTEXT__?: {
      accessToken?: string
      token?: string
    }
    __VOS_ACCESS_TOKEN__?: string
  }
}

const CLIENT_ID = 'com.ictrek.v-scheduler'
const SCOPE = 'openid profile email'
const TOKEN_CACHE_KEY = 'v_scheduler_vos_token'

let tokenPromise: Promise<string | null> | null = null
let memoryToken: { access_token: string; expires_at: number } | null = null
// Limit authorization prompts and never reuse the credential rejected by the server.
let lastAuthorizeAt = 0
let rejectedToken: string | null = null
const AUTHORIZE_MIN_INTERVAL_MS = 30_000

export async function getVOSBearerToken(): Promise<string | null> {
  if (tokenPromise) return tokenPromise
  tokenPromise = resolveVOSBearerToken().finally(() => {
    tokenPromise = null
  })
  return tokenPromise
}

export function getCachedVOSBearerToken(): string | null {
  return readCachedToken()
}

export function clearCachedVOSBearerToken(failedToken?: string) {
  rejectedToken = failedToken || memoryToken?.access_token || null
  if (!failedToken || memoryToken?.access_token === failedToken) memoryToken = null
  try {
    sessionStorage.removeItem(TOKEN_CACHE_KEY)
    localStorage.removeItem(TOKEN_CACHE_KEY)
  } catch {
    // Best-effort cache cleanup.
  }
}

async function resolveVOSBearerToken(): Promise<string | null> {
  {
    const injected = currentHostToken()
    if (injected) {
      rememberToken({ access_token: injected, expires_in: 3600 })
      return injected
    }
  }

  const cached = readCachedToken()
  if (cached) return cached

  const fastpath = await requestFastpathToken()
  if (fastpath) {
    cacheToken(fastpath)
    return fastpath.access_token
  }

  {
    const injected = currentHostToken()
    if (injected) {
      rememberToken({ access_token: injected, expires_in: 3600 })
      return injected
    }
  }
  return null
}

function usableToken(token: string | null): token is string {
  if (!token || token === rejectedToken) return false
  const expiry = tokenExpiry(token)
  return expiry === null || Date.now() + 30_000 < expiry
}
function tokenExpiry(token: string): number | null {
  try {
    const payload = JSON.parse(atob(token.split('.')[1].replace(/-/g, '+').replace(/_/g, '/')))
    return typeof payload.exp === 'number' ? payload.exp * 1000 : null
  } catch { return null }
}
function currentHostToken(): string | null {
  // Platform storage is refreshed in place; the iframe injection may be an old snapshot.
  const stored = tokenFromLocalStorage()
  if (usableToken(stored)) return stored
  const injected = tokenFromInjectedContext()
  return usableToken(injected) ? injected : null
}
function readCachedToken(): string | null {
  return memoryToken && usableToken(memoryToken.access_token) && Date.now() < memoryToken.expires_at ? memoryToken.access_token : null
}
function cacheToken(token: VOSTokenSet) { rememberToken(token) }
function rememberToken(token: VOSTokenSet) {
  const ttl = Math.max(1, Number(token.expires_in || 3600) - 60)
  memoryToken = { access_token: token.access_token, expires_at: Math.min(Date.now() + ttl * 1000, tokenExpiry(token.access_token) ?? Infinity) }
}

async function requestFastpathToken(): Promise<VOSTokenSet | null> {
  // 限频：授权流程会弹出"授权确认"窗口，token 失效风暴时不反复打扰用户，
  // 冷却期内直接返回 null（调用方展示错误，稍后重试）。
  if (Date.now() - lastAuthorizeAt < AUTHORIZE_MIN_INTERVAL_MS) return null
  lastAuthorizeAt = Date.now()

  const oauth2 = await waitForOAuth2()
  if (!oauth2) return null

  const verifier = randomBase64url()
  const state = randomBase64url()
  const nonce = randomBase64url()
  const codeChallenge = await sha256Base64url(verifier)
  const auth = await oauth2.authorize({
    client_id: CLIENT_ID,
    response_type: 'code',
    scope: SCOPE,
    state,
    code_challenge: codeChallenge,
    code_challenge_method: 'S256',
    nonce,
  })
  if (auth.state !== state) throw new Error('VOS OIDC state mismatch')
  const token = await oauth2.token({
    grant_type: 'authorization_code',
    code: auth.code,
    code_verifier: verifier,
    client_id: CLIENT_ID,
  })
  return token?.access_token ? token : null
}

async function waitForOAuth2(timeoutMs = 3000): Promise<VOSOAuth2 | null> {
  const existing = window.vos_platform?.api?.v1000?.oauth2
  if (isOAuth2(existing)) return existing

  const start = Date.now()
  return new Promise(resolve => {
    const timer = window.setInterval(() => {
      const oauth2 = window.vos_platform?.api?.v1000?.oauth2
      if (isOAuth2(oauth2)) {
        window.clearInterval(timer)
        resolve(oauth2)
      } else if (Date.now() - start >= timeoutMs) {
        window.clearInterval(timer)
        resolve(null)
      }
    }, 50)
  })
}

function isOAuth2(value: VOSOAuth2 | undefined): value is VOSOAuth2 {
  return typeof value?.authorize === 'function' && typeof value?.token === 'function'
}

function tokenFromInjectedContext(): string | null {
  const token = window.__VOS_APP_CONTEXT__?.accessToken ||
    window.__VOS_APP_CONTEXT__?.token ||
    window.__VOS_ACCESS_TOKEN__
  return typeof token === 'string' && token.trim() ? token.trim() : null
}

function tokenFromLocalStorage(): string | null {
  try {
    const directKeys = ['core-access', 'VIVIBIT-core-access']
    for (const key of directKeys) {
      const token = tokenFromAccessStore(localStorage.getItem(key))
      if (usableToken(token)) return token
    }
    for (let i = 0; i < localStorage.length; i += 1) {
      const key = localStorage.key(i)
      if (!key || !key.endsWith('-core-access')) continue
      const token = tokenFromAccessStore(localStorage.getItem(key))
      if (usableToken(token)) return token
    }
  } catch {
    return null
  }
  return null
}

function tokenFromAccessStore(raw: string | null): string | null {
  if (!raw) return null
  try {
    const data = JSON.parse(raw)
    const token = data?.accessToken || data?.access_token || data?.token
    return typeof token === 'string' && token.trim() ? token.trim() : null
  } catch {
    return null
  }
}

function randomBase64url(byteLength = 32): string {
  const bytes = new Uint8Array(byteLength)
  crypto.getRandomValues(bytes)
  return base64url(bytes)
}

async function sha256Base64url(input: string): Promise<string> {
  const bytes = new TextEncoder().encode(input)
  const digest = await crypto.subtle.digest('SHA-256', bytes)
  return base64url(new Uint8Array(digest))
}

function base64url(bytes: Uint8Array): string {
  let raw = ''
  for (let i = 0; i < bytes.length; i += 1) {
    raw += String.fromCharCode(bytes[i])
  }
  return btoa(raw).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}
