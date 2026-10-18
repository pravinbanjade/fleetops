# Single sign-on (Enterprise)

FleetOps Enterprise lets people sign in with the account they already have
at Microsoft Entra ID, Google Workspace, GitHub, any OpenID Connect
provider (Okta, Keycloak, Authentik, Auth0, …), or an LDAP directory
(Active Directory, OpenLDAP). FleetOps creates their
account the first time they sign in. Their groups at the provider decide
which teams they are in, and whether they are a FleetOps admin.

## Before you start

1. Set `FLEETOPS_PUBLIC_URL` (or `--public-url`) to the address people use
   to reach FleetOps, for example `https://fleet.example.com`. The provider
   sends people back to `<public URL>/api/v1/ee/sso/<id>/callback`. A
   redirect connection can't be enabled until this is set. LDAP doesn't
   need it.
2. Keep a **local admin** account. It always signs in with its password, so
   it is the way in when the provider is down or misconfigured.
3. Create the teams people should land in (**Admin → Teams**) and give
   those teams environment access.

## Add a connection

In **Admin → Single sign-on**, choose **New connection**, pick the
provider and fill in the fields below. After saving, open **Edit** to copy
the **Callback URL** into the provider. Then add **Mappings**, use **Test**,
and **Enable** the connection. Its "Sign in with …" button then appears on
the login page.

| Setting | Meaning |
|---|---|
| Only people in a mapped group may sign in | Off by default. When on, a sign-in from someone whose groups match no mapping is refused, and no account is created |
| Session length | How long a sign-in lasts, 1 to 720 hours (12 by default). Someone removed at the provider loses FleetOps within this time |

Secrets (client secrets, service account keys) are encrypted with the server
key (`FLEETOPS_SECRET_KEY`) and never shown again. To keep a saved secret
when editing, leave its field empty.

### Microsoft Entra ID

1. In the Entra admin center, go to **App registrations → New
   registration**. Choose *Accounts in this organizational directory only*
   and add a **Web** redirect URI: the FleetOps callback URL.
2. Under **Certificates & secrets**, create a client secret.
3. Under **Token configuration**, choose **Add groups claim** and select
   *Security groups*.
4. In FleetOps, enter the **tenant ID** (or your verified domain), the
   application (client) ID and the secret. The multi-tenant values
   `common`, `organizations` and `consumers` are refused.

Groups arrive as **object IDs**, so map those IDs (not group names). When
someone is in more than 200 groups, Entra leaves groups out of the token and
FleetOps asks Microsoft Graph instead. That needs the delegated
**GroupMember.Read.All** permission, with admin consent.

### Google Workspace

1. In Google Cloud, go to **APIs & Services → Credentials → Create
   credentials → OAuth client ID**, choose *Web application* and add the
   FleetOps callback URL as an authorized redirect URI.
2. In FleetOps, enter the client ID and secret. Under **Allowed domains**,
   list your Workspace domains (at least one). Only verified accounts in
   those domains can sign in.

Google doesn't put groups in its sign-in token. To map groups, give
FleetOps a service account that reads them:

1. Create a service account, then create a **JSON key** for it.
2. Enable the **Admin SDK API** in the same project.
3. In the Google Admin console, go to **Security → API controls →
   Domain-wide delegation**. Add the service account's client ID with the
   scope `https://www.googleapis.com/auth/admin.directory.group.readonly`.
4. In FleetOps, paste the JSON key, and enter the email of a Workspace
   admin for the service account to act as.

Groups are the group email addresses (for example `ops@example.com`), and
only direct membership counts. **Test** with an email address lists that
person's groups. If reading groups fails, sign-in still works but the
person gets no groups, and the failure is in the audit log.

### GitHub

1. In GitHub, go to **Settings → Developer settings → OAuth Apps → New
   OAuth App**. For an organisation-owned app, use the organisation's
   settings. Set the **Authorization callback URL** to the FleetOps
   callback URL.
2. In FleetOps, enter the client ID and secret and list the **allowed
   organisations**. Only members of at least one of them can sign in.
3. For GitHub Enterprise Server, enter its URL, for example
   `https://github.example.com`.

Groups are teams in the allowed organisations, written `org/team-slug` in
lower case, for example `acme/platform`. If an organisation restricts OAuth
app access, an owner must approve the app.

### Any OpenID Connect provider

Register FleetOps as a confidential client (authorization code flow) with
the callback URL as the redirect URI. Then enter the provider's **issuer
URL**. FleetOps reads `<issuer>/.well-known/openid-configuration`. Set the
**groups claim** to the ID-token claim that lists groups (`groups` by
default) and configure the provider to include it. Some providers need an
extra scope for that claim; add it under **Scopes**.

### LDAP and Active Directory

People sign in on the normal username and password form with their
directory account; there is no button. One LDAP connection is supported.

Pick the **Directory**; it fills in the filters:

| Directory | User filter | Groups |
|---|---|---|
| Active Directory | `(&(objectClass=user)(sAMAccountName={username}))` | nested, with `(member:1.2.840.113556.1.4.1941:={user_dn})` |
| OpenLDAP | `(&(objectClass=inetOrgPerson)(uid={username}))` | direct `groupOfNames` members and `posixGroup` `memberUid`s |
| Custom | your own, with `{username}` | your own, with `{user_dn}` and/or `{username}` |

Values put into a filter are escaped, so `*` or `)` in a username match
nothing special.

- **Server URL:** `ldap://dc.example.com` or `ldaps://dc.example.com:636`.
  With `ldap://`, turn on **StartTLS** unless the network is trusted.
- **CA certificate:** paste the PEM of the CA that signed the server's
  certificate when it isn't publicly trusted. **Skip TLS verification**
  exists for testing only: passwords can then be read on the network.
- **Bind DN** and **Bind password:** a read-only service account that can
  search users and groups. Leave the bind DN empty to search anonymously.
- **User base DN:** where users are searched (the whole subtree).
- **Email attribute:** `mail` by default.
- **Group base DN:** where groups are searched. Empty reads no groups.

At sign-in FleetOps binds as the service account, looks the user up (the
filter must match exactly one entry), reads their groups, then binds as the
user with the password they typed. An empty password is always refused.
Groups map by their **DN**, for example
`CN=FleetOps Ops,OU=Groups,DC=example,DC=com` (case doesn't matter).

A local account always signs in first: when a local `alice` exists and
her password matches, she is the local account. A directory `alice` whose
password doesn't match the local one becomes a separate account
`alice-2`.

If the directory can't be reached, sign-in fails like a wrong password and
the failure is written to the audit log. Local accounts keep working.

**Test** binds as the service account. Type a username to see the entry
it finds and the groups it would map; Test never asks for a password.

**Checking Active Directory by hand:** with the Active Directory preset,
run **Test** with a username that is in a nested group. The groups listed
should include the outer group. Then sign in as that person and check
**Admin → Users** shows *Signed in with …* and the mapped teams.

## Mappings

A mapping gives everyone in a group one of these:

- **Team** *name*: they join that team, and get its environment access.
- **FleetOps admin**: they get the admin role.

Group values match case-insensitively. For LDAP they are group DNs. A connection's provider (OIDC
issuer, Microsoft tenant, GitHub Enterprise URL) can't change once people
have signed in through it; add a new connection instead. Sign-ins re-check groups:

- Someone who joins a mapped group joins its team at their next sign-in.
- Someone who leaves the group leaves the team at their next sign-in.
- Removing a mapping takes its teams away from everyone straight away.
- Disabling or deleting a connection removes the teams it gave, removes
  the admin role from anyone it made an admin, and logs its users out.

FleetOps never demotes the last admin.

A user that single sign-on manages:

- shows **Signed in with …** on the Users page;
- has their role set only by mappings;
- has no password.

Team memberships an admin adds by hand are never removed by sync. Ones
that sync added show **via** *connection* in the team's members, and leave
the team when the person leaves the group.

## Password login

**Allow password login** offers two choices:

- **Everyone** (the default): local accounts keep their passwords.
- **Local admins only**: other local accounts can no longer sign in with a
  password. This setting can't be turned on until a local admin exists. If
  no local admin is left later, FleetOps ignores the limit, so nobody is
  locked out.

## Usernames

A new user's username comes from the first of these that exists:

1. the provider's preferred username;
2. the part of their email before `@`;
3. their GitHub login or LDAP username.

Characters FleetOps doesn't allow become `_`. When the name is taken, a
suffix is added: `jane-2`, then `jane-3`. A local account and a
single sign-on account are always separate, even with the same name.

## Without a license

When the license lapses:

- People who already signed in keep doing so, and their groups keep
  syncing.
- Nobody new is let in. They see *New sign-ins are paused until the
  Enterprise license is renewed.*
- You can still disable or delete connections and mappings, and set
  password login back to Everyone.
- You can't add or enable anything.

## Messages on the login page

| Message | Why |
|---|---|
| Your account isn't allowed to sign in… | The provider refused, or *Only people in a mapped group* is on and none of their groups is mapped |
| That account isn't from an allowed organisation or domain. | GitHub organisation, Google domain, Microsoft tenant or unverified Google email |
| FleetOps has reached its licensed user limit. | The license's user cap is reached; existing users still sign in |
| New sign-ins are paused… | No valid license; existing users still sign in |
| That sign-in option is turned off. | The connection is disabled |
| Sign-in failed… | Anything else: an expired or reused sign-in link, a token that failed verification, a provider error. The audit log (`sso.login`) has the detail |
