import { UserButton, useAuth } from "@clerk/react";
import { T, useMessages } from "gt-react";
import { LogInIcon, RotateCwIcon } from "lucide-react";

import { hasCloudPublicConfig } from "../../cloud/publicConfig";
import { SidebarMenu, SidebarMenuButton, SidebarMenuItem } from "../ui/sidebar";
import { retryClerkLoad, useClerkLoadFailed } from "./clerkLoadFailure";
import { LEGION_CONNECT_ACCOUNT_PAGES } from "./LegionConnectAccountPages";
import { useLegionConnectAuthPrompt } from "./useLegionConnectAuthPrompt";

export function LegionConnectSidebarSignIn() {
  if (!hasCloudPublicConfig()) return null;

  return <ConfiguredLegionConnectSidebarSignIn />;
}

export function LegionConnectSidebarAvatar() {
  if (!hasCloudPublicConfig()) return null;

  return <ConfiguredLegionConnectSidebarAvatar />;
}

function ConfiguredLegionConnectSidebarAvatar() {
  const { isLoaded, isSignedIn } = useAuth();
  const m = useMessages();

  if (!isLoaded || !isSignedIn) return null;

  return (
    <UserButton
      appearance={{
        elements: {
          avatarBox: "size-7",
          // Clerk's unlayered trigger reset (padding, radius, background) beats layered utilities.
          userButtonTrigger: "mb-1! rounded-lg! p-1! hover:bg-sidebar-row-hover!",
        },
      }}
    >
      {LEGION_CONNECT_ACCOUNT_PAGES.map((page) => (
        <UserButton.UserProfilePage
          key={page.url}
          label={m(page.label)}
          labelIcon={page.icon}
          url={page.url}
        >
          {page.content}
        </UserButton.UserProfilePage>
      ))}
    </UserButton>
  );
}

function ConfiguredLegionConnectSidebarSignIn() {
  const { isLoaded, isSignedIn } = useAuth();
  const loadFailed = useClerkLoadFailed();
  const { openAuthPrompt } = useLegionConnectAuthPrompt();

  if (loadFailed) {
    return (
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton onClick={retryClerkLoad}>
            <RotateCwIcon />
            <span>
              <T>Retry Legion Connect</T>
            </span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    );
  }
  if (!isLoaded || isSignedIn) return null;

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <SidebarMenuButton onClick={openAuthPrompt}>
          <LogInIcon />
          <span>
            <T>Sign in to Legion Connect</T>
          </span>
        </SidebarMenuButton>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}
