package limbo

import (
	"context"
	"database/sql"
	"database/sql/driver"
	"errors"
	"fmt"
	"sync"

	"github.com/ebitengine/purego"
)

func init() {
	err := ensureLibLoaded()
	if err != nil {
		panic(err)
	}
	sql.Register(driverName, &limboDriver{})
}

type limboDriver struct {
	sync.Mutex
}

var (
	libOnce           sync.Once
	limboLib          uintptr
	loadErr           error
	dbOpen            func(string) uintptr
	dbClose           func(uintptr) uintptr
	connPrepare       func(uintptr, string) uintptr
	connGetError      func(uintptr) uintptr
	freeBlobFunc      func(uintptr)
	freeStringFunc    func(uintptr)
	rowsGetColumns    func(uintptr) int32
	rowsGetColumnName func(uintptr, int32) uintptr
	rowsGetValue      func(uintptr, int32) uintptr
	rowsGetError      func(uintptr) uintptr
	closeRows         func(uintptr) uintptr
	rowsNext          func(uintptr) uintptr
	stmtQuery         func(stmtPtr uintptr, argsPtr uintptr, argCount uint64) uintptr
	stmtExec          func(stmtPtr uintptr, argsPtr uintptr, argCount uint64, changes uintptr) int32
	stmtParamCount    func(uintptr) int32
	stmtGetError      func(uintptr) uintptr
	stmtClose         func(uintptr) int32
)

// Register all the symbols on library load
func ensureLibLoaded() error {
	libOnce.Do(func() {
		limboLib, loadErr = loadLibrary()
		if loadErr != nil {
			return
		}
		purego.RegisterLibFunc(&dbOpen, limboLib, FfiDbOpen)
		purego.RegisterLibFunc(&dbClose, limboLib, FfiDbClose)
		purego.RegisterLibFunc(&connPrepare, limboLib, FfiDbPrepare)
		purego.RegisterLibFunc(&connGetError, limboLib, FfiDbGetError)
		purego.RegisterLibFunc(&freeBlobFunc, limboLib, FfiFreeBlob)
		purego.RegisterLibFunc(&freeStringFunc, limboLib, FfiFreeCString)
		purego.RegisterLibFunc(&rowsGetColumns, limboLib, FfiRowsGetColumns)
		purego.RegisterLibFunc(&rowsGetColumnName, limboLib, FfiRowsGetColumnName)
		purego.RegisterLibFunc(&rowsGetValue, limboLib, FfiRowsGetValue)
		purego.RegisterLibFunc(&closeRows, limboLib, FfiRowsClose)
		purego.RegisterLibFunc(&rowsNext, limboLib, FfiRowsNext)
		purego.RegisterLibFunc(&rowsGetError, limboLib, FfiRowsGetError)
		purego.RegisterLibFunc(&stmtQuery, limboLib, FfiStmtQuery)
		purego.RegisterLibFunc(&stmtExec, limboLib, FfiStmtExec)
		purego.RegisterLibFunc(&stmtParamCount, limboLib, FfiStmtParameterCount)
		purego.RegisterLibFunc(&stmtGetError, limboLib, FfiStmtGetError)
		purego.RegisterLibFunc(&stmtClose, limboLib, FfiStmtClose)
	})
	return loadErr
}

func (d *limboDriver) Open(name string) (driver.Conn, error) {
	d.Lock()
	conn, err := openConn(name)
	d.Unlock()
	if err != nil {
		return nil, err
	}
	return conn, nil
}

type limboConn struct {
	sync.Mutex
	ctx uintptr
}

func openConn(dsn string) (*limboConn, error) {
	ctx := dbOpen(dsn)
	if ctx == 0 {
		return nil, fmt.Errorf("failed to open database for dsn=%q", dsn)
	}
	return &limboConn{
		sync.Mutex{},
		ctx,
	}, loadErr
}

func (c *limboConn) Close() error {
	if c.ctx == 0 {
		return nil
	}
	c.Lock()
	dbClose(c.ctx)
	c.Unlock()
	c.ctx = 0
	return nil
}

func (c *limboConn) getError() error {
	if c.ctx == 0 {
		return errors.New("connection closed")
	}
	err := connGetError(c.ctx)
	if err == 0 {
		return nil
	}
	defer freeStringFunc(err)
	cpy := fmt.Sprintf("%s", GoString(err))
	return errors.New(cpy)
}

func (c *limboConn) Prepare(query string) (driver.Stmt, error) {
	if c.ctx == 0 {
		return nil, errors.New("connection closed")
	}
	c.Lock()
	defer c.Unlock()
	stmtPtr := connPrepare(c.ctx, query)
	if stmtPtr == 0 {
		return nil, c.getError()
	}
	return newStmt(stmtPtr, query), nil
}

// limboTx implements driver.Tx
type limboTx struct {
	conn *limboConn
}

// Begin starts a new transaction with default isolation level
func (c *limboConn) Begin() (driver.Tx, error) {
	c.Lock()
	defer c.Unlock()

	if c.ctx == 0 {
		return nil, errors.New("connection closed")
	}

	// Execute BEGIN statement
	stmtPtr := connPrepare(c.ctx, "BEGIN")
	if stmtPtr == 0 {
		return nil, c.getError()
	}

	stmt := newStmt(stmtPtr, "BEGIN")
	defer stmt.Close()

	_, err := stmt.Exec(nil)
	if err != nil {
		return nil, err
	}

	return &limboTx{conn: c}, nil
}

// BeginTx starts a transaction with the specified options.
// Currently only supports default isolation level and non-read-only transactions.
func (c *limboConn) BeginTx(ctx context.Context, opts driver.TxOptions) (driver.Tx, error) {
	// Skip handling non-default isolation levels and read-only mode
	// for now, letting database/sql package handle these cases
	if opts.Isolation != driver.IsolationLevel(sql.LevelDefault) || opts.ReadOnly {
		return nil, driver.ErrSkip
	}

	// Check for context cancellation
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	default:
		return c.Begin()
	}
}

// Commit commits the transaction
func (tx *limboTx) Commit() error {
	tx.conn.Lock()
	defer tx.conn.Unlock()

	if tx.conn.ctx == 0 {
		return errors.New("connection closed")
	}

	stmtPtr := connPrepare(tx.conn.ctx, "COMMIT")
	if stmtPtr == 0 {
		return tx.conn.getError()
	}

	stmt := newStmt(stmtPtr, "COMMIT")
	defer stmt.Close()

	_, err := stmt.Exec(nil)
	return err
}

// Rollback aborts the transaction.
// Note: This operation is not currently fully supported by Limbo and will return an error.
func (tx *limboTx) Rollback() error {
	tx.conn.Lock()
	defer tx.conn.Unlock()

	if tx.conn.ctx == 0 {
		return errors.New("connection closed")
	}

	stmtPtr := connPrepare(tx.conn.ctx, "ROLLBACK")
	if stmtPtr == 0 {
		return tx.conn.getError()
	}

	stmt := newStmt(stmtPtr, "ROLLBACK")
	defer stmt.Close()

	_, err := stmt.Exec(nil)
	return err
}
