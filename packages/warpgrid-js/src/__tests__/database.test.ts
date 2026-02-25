import { describe, it, expect, vi, beforeEach } from "vitest";
import { WarpGridDatabase } from "../database.js";
import { WarpGridError } from "../errors.js";
import type { DatabaseProxyBindings } from "../types.js";

function createMockBindings(
  overrides: Partial<DatabaseProxyBindings> = {},
): DatabaseProxyBindings {
  return {
    connect: vi.fn().mockReturnValue(1n),
    send: vi.fn().mockReturnValue(10),
    recv: vi.fn().mockReturnValue(new Uint8Array([1, 2, 3])),
    close: vi.fn(),
    ...overrides,
  };
}

describe("WarpGridDatabase", () => {
  let bindings: DatabaseProxyBindings;
  let db: WarpGridDatabase;

  beforeEach(() => {
    bindings = createMockBindings();
    db = new WarpGridDatabase(bindings);
  });

  describe("connect()", () => {
    it("returns a connection object with send, recv, and close methods", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      expect(conn).toBeDefined();
      expect(typeof conn.send).toBe("function");
      expect(typeof conn.recv).toBe("function");
      expect(typeof conn.close).toBe("function");
    });

    it("calls the WIT connect binding with correct config", () => {
      db.connect({
        host: "db.internal",
        port: 5432,
        database: "mydb",
        username: "appuser",
        password: "secret",
      });

      expect(bindings.connect).toHaveBeenCalledWith({
        host: "db.internal",
        port: 5432,
        database: "mydb",
        user: "appuser",
        password: "secret",
      });
    });

    it("maps 'username' to 'user' for the WIT binding", () => {
      db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "john",
      });

      expect(bindings.connect).toHaveBeenCalledWith(
        expect.objectContaining({ user: "john" }),
      );
    });

    it("passes undefined password when not provided", () => {
      db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      expect(bindings.connect).toHaveBeenCalledWith(
        expect.objectContaining({ password: undefined }),
      );
    });

    it("throws WarpGridError when host is missing", () => {
      expect(() =>
        db.connect({
          host: "",
          port: 5432,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(WarpGridError);
    });

    it("throws WarpGridError when host is missing with descriptive message", () => {
      expect(() =>
        db.connect({
          host: "",
          port: 5432,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(/host/i);
    });

    it("throws WarpGridError when database is missing", () => {
      expect(() =>
        db.connect({
          host: "localhost",
          port: 5432,
          database: "",
          username: "admin",
        }),
      ).toThrow(WarpGridError);
    });

    it("throws WarpGridError when username is missing", () => {
      expect(() =>
        db.connect({
          host: "localhost",
          port: 5432,
          database: "testdb",
          username: "",
        }),
      ).toThrow(WarpGridError);
    });

    it("throws WarpGridError when port is out of range", () => {
      expect(() =>
        db.connect({
          host: "localhost",
          port: 0,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(WarpGridError);

      expect(() =>
        db.connect({
          host: "localhost",
          port: 70000,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(WarpGridError);
    });

    it("wraps WIT binding errors in WarpGridError", () => {
      const failingBindings = createMockBindings({
        connect: vi.fn().mockImplementation(() => {
          throw "connection refused";
        }),
      });
      const failDb = new WarpGridDatabase(failingBindings);

      expect(() =>
        failDb.connect({
          host: "localhost",
          port: 5432,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(WarpGridError);
    });

    it("includes original error as cause when WIT binding fails", () => {
      const failingBindings = createMockBindings({
        connect: vi.fn().mockImplementation(() => {
          throw "pool exhausted";
        }),
      });
      const failDb = new WarpGridDatabase(failingBindings);

      try {
        failDb.connect({
          host: "localhost",
          port: 5432,
          database: "testdb",
          username: "admin",
        });
        expect.fail("should have thrown");
      } catch (err) {
        expect(err).toBeInstanceOf(WarpGridError);
        expect((err as WarpGridError).cause).toBeDefined();
      }
    });
  });

  describe("Connection.send()", () => {
    it("forwards data to the WIT send binding", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      const data = new Uint8Array([0x51, 0x00, 0x00, 0x0d]);
      conn.send(data);

      expect(bindings.send).toHaveBeenCalledWith(1n, data);
    });

    it("returns void (discards byte count from WIT)", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      const result = conn.send(new Uint8Array([1, 2, 3]));
      expect(result).toBeUndefined();
    });

    it("wraps WIT send errors in WarpGridError", () => {
      const failingBindings = createMockBindings({
        send: vi.fn().mockImplementation(() => {
          throw "broken pipe";
        }),
      });
      const failDb = new WarpGridDatabase(failingBindings);
      const conn = failDb.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      expect(() => conn.send(new Uint8Array([1]))).toThrow(WarpGridError);
    });
  });

  describe("Connection.recv()", () => {
    it("calls the WIT recv binding with the connection handle and maxBytes", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      conn.recv(1024);

      expect(bindings.recv).toHaveBeenCalledWith(1n, 1024);
    });

    it("returns the Uint8Array from the WIT binding", () => {
      const expected = new Uint8Array([0x52, 0x00, 0x00, 0x08]);
      const customBindings = createMockBindings({
        recv: vi.fn().mockReturnValue(expected),
      });
      const customDb = new WarpGridDatabase(customBindings);
      const conn = customDb.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      const result = conn.recv(1024);
      expect(result).toEqual(expected);
    });

    it("wraps WIT recv errors in WarpGridError", () => {
      const failingBindings = createMockBindings({
        recv: vi.fn().mockImplementation(() => {
          throw "read timeout";
        }),
      });
      const failDb = new WarpGridDatabase(failingBindings);
      const conn = failDb.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      expect(() => conn.recv(1024)).toThrow(WarpGridError);
    });
  });

  describe("Connection.close()", () => {
    it("calls the WIT close binding with the connection handle", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      conn.close();

      expect(bindings.close).toHaveBeenCalledWith(1n);
    });

    it("wraps WIT close errors in WarpGridError", () => {
      const failingBindings = createMockBindings({
        close: vi.fn().mockImplementation(() => {
          throw "invalid handle";
        }),
      });
      const failDb = new WarpGridDatabase(failingBindings);
      const conn = failDb.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      expect(() => conn.close()).toThrow(WarpGridError);
    });
  });

  describe("use-after-close protection", () => {
    it("throws WarpGridError when send() is called after close()", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      conn.close();

      expect(() => conn.send(new Uint8Array([1]))).toThrow(WarpGridError);
      expect(() => conn.send(new Uint8Array([1]))).toThrow(/closed/);
    });

    it("throws WarpGridError when recv() is called after close()", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      conn.close();

      expect(() => conn.recv(1024)).toThrow(WarpGridError);
      expect(() => conn.recv(1024)).toThrow(/closed/);
    });

    it("throws WarpGridError when close() is called twice", () => {
      const conn = db.connect({
        host: "localhost",
        port: 5432,
        database: "testdb",
        username: "admin",
      });

      conn.close();

      expect(() => conn.close()).toThrow(WarpGridError);
      expect(() => conn.close()).toThrow(/closed/);
    });
  });

  describe("port validation edge cases", () => {
    it("throws WarpGridError for NaN port", () => {
      expect(() =>
        db.connect({
          host: "localhost",
          port: NaN,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(WarpGridError);
    });

    it("throws WarpGridError for non-integer port", () => {
      expect(() =>
        db.connect({
          host: "localhost",
          port: 5432.7,
          database: "testdb",
          username: "admin",
        }),
      ).toThrow(WarpGridError);
    });
  });

  describe("multiple connections", () => {
    it("assigns unique handles to each connection", () => {
      const multiBindings = createMockBindings({
        connect: vi
          .fn()
          .mockReturnValueOnce(1n)
          .mockReturnValueOnce(2n),
      });
      const multiDb = new WarpGridDatabase(multiBindings);

      const conn1 = multiDb.connect({
        host: "localhost",
        port: 5432,
        database: "db1",
        username: "admin",
      });
      const conn2 = multiDb.connect({
        host: "localhost",
        port: 5432,
        database: "db2",
        username: "admin",
      });

      conn1.send(new Uint8Array([1]));
      conn2.send(new Uint8Array([2]));

      // First connection uses handle 1n, second uses 2n
      expect(multiBindings.send).toHaveBeenCalledWith(
        1n,
        new Uint8Array([1]),
      );
      expect(multiBindings.send).toHaveBeenCalledWith(
        2n,
        new Uint8Array([2]),
      );
    });
  });
});
