CREATE TRIGGER used_invitation_nonces_no_update
BEFORE UPDATE ON used_invitation_nonces BEGIN
    SELECT RAISE(ABORT, 'spent invitation bindings are append-only');
END;

CREATE TRIGGER used_invitation_nonces_no_delete
BEFORE DELETE ON used_invitation_nonces BEGIN
    SELECT RAISE(ABORT, 'spent invitation bindings are append-only');
END;
