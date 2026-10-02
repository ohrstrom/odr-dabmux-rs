import * as React from "react"

/** Hash-based route, split into segments: "#/services/4DA4" → ["services", "4DA4"]. */
export function useRoute(): string[] {
  const [hash, setHash] = React.useState(() => window.location.hash)

  React.useEffect(() => {
    const onChange = () => setHash(window.location.hash)
    window.addEventListener("hashchange", onChange)
    return () => window.removeEventListener("hashchange", onChange)
  }, [])

  return hash.replace(/^#\/?/, "").split("/").filter(Boolean)
}

export function href(...segments: string[]): string {
  return `#/${segments.join("/")}`
}
