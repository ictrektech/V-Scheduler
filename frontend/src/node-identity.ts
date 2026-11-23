type Address = { hostname?: string | null, address?: string | null, profile?: string | null }
type NamedNode = Address & {
 id?: string
 name?: string | null
 host?: string | null
 display_name?: string | null
 modelinfer_source?: { node: Address | null } | null
 vos_node?: { display_name?: string | null, node_ip?: string | null } | null
 vos_metrics?: { data: { hostname?: string | null, device_name?: string | null } } | null
}
export function nodeIdentity(node?: NamedNode | null) {
 const source = node?.modelinfer_source?.node
 return {
  name: source?.hostname?.trim() || node?.hostname?.trim() || node?.vos_metrics?.data.hostname?.trim()
   || node?.name?.trim() || node?.display_name?.trim() || node?.vos_node?.display_name?.trim()
   || node?.vos_metrics?.data.device_name?.trim() || '',
  address: source?.address || node?.address || node?.host || node?.vos_node?.node_ip || '',
  profile: source?.profile || node?.profile || '',
 }
}
