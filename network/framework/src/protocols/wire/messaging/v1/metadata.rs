// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use crate::{
    counters,
    protocols::wire::{
        handshake::v1::ProtocolId,
        messaging::v1::{MultiplexMessage, NetworkMessage, RpcResponse},
    },
};
use aptos_config::network_id::NetworkId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// A simple struct that wraps a network message with metadata.
/// Note: this is not sent along the wire, it is only used internally.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct NetworkMessageWithMetadata {
    /// The metadata about the message
    message_metadata: MessageMetadata,

    /// The network message to send along the wire
    network_message: NetworkMessage,
}

impl NetworkMessageWithMetadata {
    pub fn new(message_metadata: MessageMetadata, network_message: NetworkMessage) -> Self {
        Self {
            message_metadata,
            network_message,
        }
    }

    /// Converts the message into a multiplex message with metadata
    pub fn into_multiplex_message(self) -> MultiplexMessageWithMetadata {
        MultiplexMessageWithMetadata::new(
            self.message_metadata,
            MultiplexMessage::Message(self.network_message),
        )
    }

    /// Consumes the message and returns the individual parts
    pub fn into_parts(self) -> (MessageMetadata, NetworkMessage) {
        (self.message_metadata, self.network_message)
    }

    /// Returns a reference to the message metadata
    pub fn network_message(&self) -> &NetworkMessage {
        &self.network_message
    }
}

/// A simple struct that wraps a multiplex message with metadata.
/// Note: this is not sent along the wire, it is only used internally.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MultiplexMessageWithMetadata {
    /// The metadata about the message
    message_metadata: MessageMetadata,

    /// The multiplex message to send along the wire
    multiplex_message: MultiplexMessage,
}

impl MultiplexMessageWithMetadata {
    pub fn new(message_metadata: MessageMetadata, multiplex_message: MultiplexMessage) -> Self {
        Self {
            message_metadata,
            multiplex_message,
        }
    }

    /// Consumes the message and returns the individual parts
    pub fn into_parts(self) -> (MessageMetadata, MultiplexMessage) {
        (self.message_metadata, self.multiplex_message)
    }
}

/// A simple struct that wraps an RPC response with metadata.
/// Note: this is not sent along the wire, it is only used internally.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RpcResponseWithMetadata {
    /// The metadata about the message
    message_metadata: MessageMetadata,

    /// The response to send along the wire
    response: RpcResponse,
}

impl RpcResponseWithMetadata {
    pub fn new(message_metadata: MessageMetadata, response: RpcResponse) -> Self {
        Self {
            message_metadata,
            response,
        }
    }

    /// Transforms the message into an RPC response network message with metadata
    pub fn into_network_message(self) -> NetworkMessageWithMetadata {
        // Create the RPC response network message
        let network_message = NetworkMessage::RpcResponse(self.response);

        // Create and return the network message with metadata
        NetworkMessageWithMetadata::new(self.message_metadata, network_message)
    }

    /// Returns a reference to the RPC response
    pub fn rpc_response(&self) -> &RpcResponse {
        &self.response
    }
}

/// A simple enum to track the message send type
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum MessageSendType {
    DirectSend,  // A direct send message to another peer
    RpcRequest,  // An RPC request to another peer
    RpcResponse, // An RPC response for a request sent by another peer
}

impl MessageSendType {
    pub fn get_label(&self) -> &'static str {
        match self {
            MessageSendType::DirectSend => "DirectSend",
            MessageSendType::RpcRequest => "RpcRequest",
            MessageSendType::RpcResponse => "RpcResponse",
        }
    }
}

/// A simple enum to track the message stream type
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum MessageStreamType {
    NonStreamedMessage,      // A non-streamed message that fits into a single chunk
    StreamedMessageHead,     // The head (first fragment) of a streamed message
    StreamedMessageFragment, // A fragment of a streamed message (not the head or tail)
    StreamedMessageTail,     // The tail (last fragment) of a streamed message
}

impl MessageStreamType {
    pub fn get_label(&self) -> &'static str {
        match self {
            MessageStreamType::NonStreamedMessage => "NonStreamedMessage",
            MessageStreamType::StreamedMessageHead => "StreamedMessageHead",
            MessageStreamType::StreamedMessageFragment => "StreamedMessageFragment",
            MessageStreamType::StreamedMessageTail => "StreamedMessageTail",
        }
    }
}

/// A struct holding metadata about each message.
/// Note: this is not sent along the wire, it is only used internally.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MessageMetadata {
    /// The network ID for the message
    network_id: NetworkId,

    /// The protocol ID for the message
    protocol_id: ProtocolId,

    /// The type of message being sent
    message_send_type: MessageSendType,

    /// The stream type of the message being sent
    message_stream_type: MessageStreamType,

    /// The time at which the message was sent by the application
    application_send_time: Option<SystemTime>,
}

impl MessageMetadata {
    pub fn new(
        network_id: NetworkId,
        protocol_id: ProtocolId,
        message_send_type: MessageSendType,
        application_send_time: Option<SystemTime>,
    ) -> Self {
        Self {
            network_id,
            protocol_id,
            message_send_type,
            message_stream_type: MessageStreamType::NonStreamedMessage, // Default to non-streamed messages
            application_send_time,
        }
    }

    /// Returns the time at which the message was first sent by the application
    pub fn application_send_time(&self) -> Option<SystemTime> {
        self.application_send_time
    }

    /// Marks the message as being sent over the network wire by emitting latency metrics
    pub fn mark_message_as_sending(&mut self) {
        // If this message is a streamed message fragment, there's no need to emit
        // any metrics (we only emit metrics for the head and tail of streamed messages).
        if self.message_stream_type == MessageStreamType::StreamedMessageFragment {
            return;
        }

        // Otherwise, emit the latency metrics
        if let Some(application_send_time) = self.application_send_time {
            // Calculate the application to wire send latency
            let application_to_wire_latency = application_send_time
                .elapsed()
                .unwrap_or_default()
                .as_secs_f64();

            // Update the application to wire latency metrics
            counters::observe_value_with_labels(
                &counters::APTOS_NETWORK_MESSAGE_SEND_LATENCY,
                &self.network_id,
                &self.protocol_id,
                &self.message_send_type,
                &self.message_stream_type,
                application_to_wire_latency,
            );
        }
    }

    /// Updates the message type
    pub fn update_message_stream_type(&mut self, message_stream_type: MessageStreamType) {
        self.message_stream_type = message_stream_type;
    }
}
