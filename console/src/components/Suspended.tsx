import { useCapabilities, useOrganization } from "../platform-data";

/** On every page of a suspended organisation: what it means and who can fix it (ADR-035). */
export function SuspendedBanner({ org }: { org: string }) {
  const o = useOrganization(org);
  const support = useCapabilities().data?.supportUrl ?? "";
  if (!o.data?.suspended) return null;
  return (
    <div className="banner" role="status">
      <p>
        <strong>{org} is suspended.</strong> Its repositories can be read, but nothing can be pushed, landed or created until it is restored.
        {!support && " Contact the people who run this server."}
      </p>
      {support && (
        <a className="button" href={support} rel="noopener">
          Contact support
        </a>
      )}
    </div>
  );
}
