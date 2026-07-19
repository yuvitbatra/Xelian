"use client";

// Session state for the website. The registry issues plain bearer tokens
// (no cookies); we keep them in localStorage and render README markdown
// only through a sanitizer, so no untrusted script runs on this origin.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";

const TOKEN_KEY = "xelian.token";
const USERNAME_KEY = "xelian.username";

interface AuthState {
  token: string | null;
  username: string | null;
  /** False until localStorage has been read on the client. */
  ready: boolean;
  signIn: (token: string, username: string) => void;
  signOut: () => void;
}

const AuthContext = createContext<AuthState>({
  token: null,
  username: null,
  ready: false,
  signIn: () => {},
  signOut: () => {},
});

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [token, setToken] = useState<string | null>(null);
  const [username, setUsername] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    setToken(localStorage.getItem(TOKEN_KEY));
    setUsername(localStorage.getItem(USERNAME_KEY));
    setReady(true);
  }, []);

  const signIn = useCallback((newToken: string, newUsername: string) => {
    localStorage.setItem(TOKEN_KEY, newToken);
    localStorage.setItem(USERNAME_KEY, newUsername);
    setToken(newToken);
    setUsername(newUsername);
  }, []);

  const signOut = useCallback(() => {
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USERNAME_KEY);
    setToken(null);
    setUsername(null);
  }, []);

  return (
    <AuthContext.Provider value={{ token, username, ready, signIn, signOut }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth(): AuthState {
  return useContext(AuthContext);
}
