import { Route, Switch, useLocation } from "wouter-preact";
import { useEffect } from "preact/hooks";
import { Shell } from "./components/Shell";
import { Login } from "./pages/Login";
import { Dashboard } from "./pages/Dashboard";
import { Peers } from "./pages/Peers";
import { PeerDetail } from "./pages/PeerDetail";
import { Logs } from "./pages/Logs";
import { Tasks } from "./pages/Tasks";
import { Chat } from "./pages/Chat";
import { Settings } from "./pages/Settings";
import { fetchAuth } from "./lib/store";
import { EmptyState } from "./components/ui";

function NotFound() {
  return (
    <EmptyState
      title="Page not found"
      hint="The console route you opened doesn't exist."
      action={
        <a href="/">
          <button class="btn">Back to dashboard</button>
        </a>
      }
    />
  );
}

export function App() {
  const [loc] = useLocation();

  useEffect(() => {
    fetchAuth();
  }, []);

  // scroll to top on navigation
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [loc]);

  if (loc === "/login") return <Login />;

  return (
    <Shell>
      <Switch>
        <Route path="/" component={Dashboard} />
        <Route path="/peers" component={Peers} />
        <Route path="/peers/:name" component={PeerDetail} />
        <Route path="/tasks" component={Tasks} />
        <Route path="/logs" component={Logs} />
        <Route path="/chat" component={Chat} />
        <Route path="/settings" component={Settings} />
        <Route component={NotFound} />
      </Switch>
    </Shell>
  );
}
