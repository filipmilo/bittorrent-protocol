# Peer Protocol


- Connections are symmetrical. Messages look the same in both directions
- When a file is downloaded the piece hash is checked to validate that the file is downloaded after that the peer announces to other peers
- Connection contain two bits (choking, interest)
- Chocking is a notification that no data will be sent
- Data transfer only if one side is **interested** and other is **not choking**
- Interested: "Does a peer have piece i need?"
- Choked: "Are they willing to send data"
- All connections start out choked and not interested
- If a peer does not have pieces my peer needs, send them 'NOT INTERESTED' even if my peer is 'CHOKED'
- If a peer has pieces my peer need, send them 'INTERESTED' even if my peer is 'CHOKED'


- We are using url percentage encoding on the info_hash field because of problematic urls
- Handshaking is done by sending a 68 len byte array to the peer, its in the following format:
  `19 + b'BitTorrent protocol' + info_hash_raw + peer_id_raw`


- Using sparse files we can insure we seek to a file location with random pieces and the space between wont be allocated.
- Use for example a json file that tracks state of downloaded pieces.
