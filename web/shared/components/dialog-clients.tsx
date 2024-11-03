import { useRef, useImperativeHandle } from 'preact/hooks';
import { forwardRef } from 'preact/compat';
import { Button, Modal, Table } from 'react-daisyui';

import { type Session, deleteSession } from '../api';
import { formatTime } from '../utils';

interface Props {
    id: string
    sessions: Session[]
}

export interface IClientsDialog {
    show(): void
}

export const ClientsDialog = forwardRef<IClientsDialog, Props>((props: Props, ref) => {
    const refDialog = useRef<HTMLDialogElement>(null);

    useImperativeHandle(ref, () => {
        return {
            show: () => {
                refDialog.current?.showModal();
            }
        };
    });

    return (
        <Modal ref={refDialog} className="min-w-md max-w-[unset] w-[unset]">
            <Modal.Header className="mb-2">
                <h3 className="font-bold">Clients of {props.id}</h3>
            </Modal.Header>
            <Modal.Body>
                <Table>
                    <Table.Head>
                        <span>ID</span>
                        <span>State</span>
                        <span>Creation Time</span>
                        <span>Operation</span>
                    </Table.Head>
                    <Table.Body>
                        {props.sessions.length > 0 ? props.sessions.map(c =>
                            <Table.Row>
                                <span>{c.id + (c.reforward ? '(reforward)' : '')}</span>
                                <span>{c.state}</span>
                                <span>{formatTime(c.createdAt)}</span>
                                <Button size="sm" color="error" onClick={() => deleteSession(props.id, c.id)}>Kick</Button>
                            </Table.Row>
                        ): <tr><td colspan={4} className="text-center">N/A</td></tr>}
                    </Table.Body>
                </Table>
            </Modal.Body>
            <Modal.Actions>
                <form method="dialog">
                    <Button>Close</Button>
                </form>
            </Modal.Actions>
        </Modal>
    );
});
